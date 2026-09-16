import 'dart:convert';

import 'package:desktop_multi_window/desktop_multi_window.dart';
import 'package:flutter_hbb/common.dart';
import 'package:flutter_hbb/common/shared_state.dart';
import 'package:flutter_hbb/consts.dart';
import 'package:flutter_hbb/models/model.dart';
import 'package:flutter_hbb/models/platform_model.dart';
import 'package:flutter_hbb/models/state_model.dart';
import 'package:flutter_hbb/utils/scale.dart';

import 'remote_toolbar.dart';

class _ViewError implements Exception {
  final String code;
  final String message;
  _ViewError(this.code, this.message);
}

/// Runs only in the destination desktop view; no synthetic toolbar clicks.
class AutomationView {
  final FFI ffi;
  final ToolbarState toolbar;
  final bool Function() mounted;
  final void Function() close;
  AutomationView(this.ffi, this.toolbar, this.mounted, this.close);

  void guard(String request) {
    if (!mounted() || !bind.automationViewGuard(
        requestId: request, sessionId: ffi.sessionId)) {
      throw _ViewError('CONTROL_EXPIRED', 'View closed, authority changed, or request expired');
    }
  }

  bool toggle(String key) => bind.sessionGetToggleOptionSync(
      sessionId: ffi.sessionId, arg: key);

  Future<void> setToggle(String request, String key, bool value) async {
    guard(request);
    if (toggle(key) != value) {
      await bind.sessionToggleOption(sessionId: ffi.sessionId, value: key);
    }
  }

  Future<Map<String, dynamic>> state() async {
    final model = ffi.ffiModel;
    final pi = model.pi;
    final screens = await getScreenRectList();
    final style = await bind.sessionGetViewStyle(sessionId: ffi.sessionId);
    final multi = bind.sessionIsMultiUiSession(sessionId: ffi.sessionId);
    final cursorSupported = pi.platform != kPeerPlatformAndroid &&
        !ffi.canvasModel.cursorEmbedded && !pi.isWayland;
    final followSupported = cursorSupported && versionCmp(pi.version, '1.2.4') >= 0 &&
        pi.displays.length > 1 && pi.currentDisplay != kAllDisplayValue && !multi;
    return {
      'ui_session_id': ffi.sessionId.toString(),
      'window_id': stateGlobal.windowId,
      'display_id': pi.currentDisplay.toString(),
      'multiple_views': multi,
      'scale': {'mode': style, 'percent': await getSessionCustomScalePercent(ffi.sessionId),
        'render_scale': ffi.canvasModel.scale, 'scope': 'peer_preference'},
      'individual_windows': bind.sessionGetDisplaysAsIndividualWindows(sessionId: ffi.sessionId) == 'Y',
      'use_all_local_displays': bind.sessionGetUseAllMyDisplaysForTheRemoteSession(sessionId: ffi.sessionId) == 'Y',
      'display_preferences_apply': {'individual_windows': 'subsequent_toolbar_selections', 'use_all_local_displays': 'next_fresh_connection'},
      'show_remote_cursor': toggle('show-remote-cursor'),
      'follow_remote_cursor': toggle('follow-remote-cursor'),
      'follow_remote_focus': toggle('follow-remote-window'),
      'scale_cursor': toggle(kOptionZoomCursor),
      'follow_ai_display': bind.mainGetLocalOption(key: 'mcp-follow-display') == 'Y',
      'toolbar_pinned': toolbar.pin,
      'fullscreen': await WindowController.fromWindowId(stateGlobal.windowId).isFullScreen(),
      'support': {'individual_windows': pi.isSupportMultiDisplay,
        'use_all_local_displays': pi.isSupportMultiDisplay && screens.length > 1,
        'show_remote_cursor': cursorSupported && !model.viewOnly,
        'follow_remote_cursor': followSupported, 'follow_remote_focus': followSupported,
        'scale_cursor': pi.platform != kPeerPlatformAndroid && style != kRemoteViewStyleOriginal},
      'effective': {'follow_remote_cursor': followSupported && toggle('follow-remote-cursor'),
        'follow_remote_focus': followSupported && toggle('follow-remote-window'),
        'scale_cursor': style != kRemoteViewStyleOriginal && toggle(kOptionZoomCursor),
        'show_remote_cursor_locked': ShowRemoteCursorLockState.find(ffi.id).value},
      'scopes': {'cursor_and_display_preferences': 'peer_preference',
        'follow_ai_display': 'global', 'toolbar_pinned': 'global_preference_current_view',
        'fullscreen': 'local_os_window_including_other_tabs'},
      'local_displays': screens.asMap().entries.map((e) => {'index': e.key,
        'x': e.value.left, 'y': e.value.top, 'width': e.value.width, 'height': e.value.height}).toList(),
    };
  }

  Future<String> set(String request, Map<String, dynamic> change) async {
    final setting = change['setting'] as String;
    final enabled = change['enabled'] == true;
    final before = await state();
    guard(request);
    final support = before['support'] as Map<String, dynamic>;
    if (enabled && support[setting] == false) {
      throw _ViewError('UNSUPPORTED', 'Setting is not supported by this platform, display layout, or view mode');
    }
    switch (setting) {
      case 'scale':
        if (change['mode'] == 'custom') {
          await bind.sessionSetFlutterOption(sessionId: ffi.sessionId,
              k: kCustomScalePercentKey, v: change['percent'].toString());
          guard(request);
        }
        await bind.sessionSetViewStyle(sessionId: ffi.sessionId, value: change['mode']);
        guard(request);
        await ffi.canvasModel.updateViewStyle(refreshMousePos: false);
        return 'peer_preference';
      case 'individual_windows':
        await bind.sessionSetDisplaysAsIndividualWindows(sessionId: ffi.sessionId, value: enabled ? 'Y' : 'N');
        return 'peer_preference';
      case 'use_all_local_displays':
        await bind.sessionSetUseAllMyDisplaysForTheRemoteSession(sessionId: ffi.sessionId, value: enabled ? 'Y' : 'N');
        return 'peer_preference';
      case 'show_remote_cursor':
        if (!enabled && before['effective']['show_remote_cursor_locked'] == true) {
          throw _ViewError('SETTING_CONFLICT', 'Disable follow_remote_cursor before hiding its cursor');
        }
        await setToggle(request, 'show-remote-cursor', enabled);
        ShowRemoteCursorState.find(ffi.id).value = toggle('show-remote-cursor');
        return 'peer_preference';
      case 'follow_remote_cursor':
        if (enabled) {
          await setToggle(request, 'show-remote-cursor', true);
          ShowRemoteCursorState.find(ffi.id).value = toggle('show-remote-cursor');
        }
        await setToggle(request, 'follow-remote-cursor', enabled);
        ShowRemoteCursorLockState.find(ffi.id).value = enabled;
        return 'peer_preference';
      case 'follow_remote_focus':
        await setToggle(request, 'follow-remote-window', enabled);
        return 'peer_preference';
      case 'scale_cursor':
        await setToggle(request, kOptionZoomCursor, enabled);
        PeerBoolOption.find(ffi.id, kOptionZoomCursor).value = toggle(kOptionZoomCursor);
        return 'peer_preference';
      case 'follow_ai_display':
        await bind.mainSetLocalOption(key: 'mcp-follow-display', value: enabled ? 'Y' : 'N');
        return 'global';
      case 'toolbar_pinned':
        await toolbar.setPin(enabled);
        return 'global_preference_current_view';
      case 'fullscreen':
        await WindowController.fromWindowId(stateGlobal.windowId).setFullscreen(enabled);
        for (var attempt = 0; attempt < 30; attempt++) {
          final actual = await WindowController.fromWindowId(stateGlobal.windowId).isFullScreen();
          guard(request);
          if (actual == enabled) {
            stateGlobal.setFullscreen(actual, procWnd: false);
            break;
          }
          await Future<void>.delayed(const Duration(milliseconds: 100));
        }
        return 'local_os_window_including_other_tabs';
      default:
        throw _ViewError('INVALID_ARGUMENT', 'Unknown view setting');
    }
  }

  Future<void> handle(String request) async {
    final raw = bind.automationViewClaim(requestId: request, sessionId: ffi.sessionId);
    if (raw.isEmpty) return;
    final command = jsonDecode(raw) as Map<String, dynamic>;
    if (command['error'] != null) return;
    Map<String, dynamic> result;
    var closeAfterReply = false;
    try {
      guard(request);
      var scope = 'local_window';
      var confirmed = true;
      var delivery = 'observed';
      switch (command['command']) {
        case 'get': break;
        case 'set':
          scope = await set(request, command['change']);
          delivery = 'applied';
          break;
        case 'window':
          final action = command['action'] as Map<String, dynamic>;
          if (action['action'] == 'show') {
            await windowOnTop(stateGlobal.windowId);
            delivery = 'applied';
          } else if (action['action'] == 'close') {
            closeAfterReply = true;
            delivery = 'sent';
            confirmed = false;
          } else if (action['action'] == 'open_display') {
            final index = int.tryParse(action['display_id']);
            if (!ffi.ffiModel.pi.isSupportMultiDisplay) {
              throw _ViewError('UNSUPPORTED', 'Peer lacks multi-view support');
            }
            if (index == null || index < 0 || index >= ffi.ffiModel.pi.displays.length) {
              throw _ViewError('DISPLAY_NOT_FOUND', 'Display is absent or offline');
            }
            guard(request);
            await openMonitorInNewTabOrWindow(index, ffi.id, ffi.ffiModel.pi,
                automationRequestId: request, automationSource: ffi.sessionId);
            delivery = 'sent';
            confirmed = false;
          }
          break;
      }
      final observed = await state();
      if (command['command'] == 'set' && command['change']['setting'] == 'fullscreen' &&
          observed['fullscreen'] != command['change']['enabled']) {
        confirmed = false;
        delivery = 'sent';
      }
      result = {'confirmed': confirmed, 'delivery': delivery, 'scope': scope, 'state': observed};
      guard(request);
    } on _ViewError catch (e) {
      closeAfterReply = false;
      result = {'error': {'code': e.code, 'message': e.message}};
    } catch (e) {
      closeAfterReply = false;
      result = {'error': {'code': 'GUI_ERROR', 'message': e.toString()}};
    }
    // The final-view close can destroy this Flutter engine; acknowledge dispatch first.
    final active = bind.automationViewComplete(requestId: request, sessionId: ffi.sessionId, result: jsonEncode(result));
    if (closeAfterReply && active && mounted()) close();
  }
}
