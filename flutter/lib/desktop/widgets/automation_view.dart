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

  Future<Map<String, dynamic>> connectionState() async {
    final model = ffi.ffiModel;
    final pi = model.pi;
    final monitor = ffi.qualityMonitorModel;
    final now = DateTime.now();
    Map<String, dynamic> metric(String field, String? value, String unit,
        {bool stable = false}) {
      if (value == '-' || value == '') value = null;
      final at = monitor.observedAt[field];
      final age = at == null ? null : now.difference(at).inMilliseconds;
      return {'value': value, 'known': value != null && at != null,
        'observed_at': at?.toUtc().toIso8601String(), 'age_ms': age,
        'fresh': value != null && age != null && (stable || age <= 10000), 'unit': unit};
    }
    final publicServer = await bind.mainIsUsingPublicServer();
    final relayLimited = publicServer && model.direct != true;
    final alternative = jsonDecode(await bind.sessionAlternativeCodecs(sessionId: ffi.sessionId)) as Map<String, dynamic>;
    final codecs = ['auto', 'vp9', ...['vp8', 'av1', 'h264', 'h265'].where((c) => alternative[c] == true)];
    final custom = await bind.sessionGetCustomImageQuality(sessionId: ffi.sessionId);
    final codec = monitor.data.codecFormat;
    final trueColorSupported = versionCmp(pi.version, '1.2.4') >= 0 &&
        (codec == 'AV1' || codec == 'VP9') && monitor.observedAt['codecFormat'] != null;
    return {
      'settings': {
        'quality': await bind.sessionGetImageQuality(sessionId: ffi.sessionId),
        'custom_quality': custom != null && custom.isNotEmpty ? custom.first : 50,
        'custom_fps': int.tryParse(await bind.sessionGetOption(sessionId: ffi.sessionId, arg: 'custom-fps') ?? '') ?? 30,
        'codec_preference': await bind.sessionGetOption(sessionId: ffi.sessionId, arg: kOptionCodecPreference),
        'true_color': toggle('i444'),
        'audio_muted': toggle('disable-audio'),
        'lock_after_end': toggle('lock-after-session-end'),
        'quality_overlay': toggle('show-quality-monitor'),
        'quality_overlay_visible': monitor.show,
      },
      'support': {
        'available_codecs': codecs,
        'codecs_known': pi.version.isNotEmpty && model.direct != null,
        'quality_min': 10,
        'quality_max': !relayLimited && versionCmp(pi.version, '1.2.2') >= 0 ? 2000 : 100,
        'custom_fps': !relayLimited && versionCmp(pi.version, '1.2.0') >= 0,
        'fps_min': 5, 'fps_max': 120,
        'public_relay_restriction': relayLimited,
        'true_color': trueColorSupported,
        'true_color_reason': trueColorSupported ? null : 'requires_peer_1_2_4_and_observed_vp9_or_av1',
        'audio_permission': pi.version.isEmpty ? null : model.permissions['audio'] != false,
        'lock_after_end': model.keyboard && !model.viewOnly && !model.isPeerAndroid,
      },
      'metrics': {
        'speed': metric('speed', monitor.data.speed, 'stock_formatted_rate'),
        'fps': metric('fps', monitor.data.fps, 'frames_per_second_current_view'),
        'delay': metric('delay', monitor.data.delay, 'ms'),
        'target_bitrate': metric('targetBitrate', monitor.data.targetBitrate, 'stock_kb'),
        'codec': metric('codecFormat', codec, 'codec', stable: true),
        'chroma': metric('chroma', monitor.data.chroma, 'chroma', stable: true),
      },
      'connection': {'secure': model.secure, 'direct': model.direct,
        'transport': model.cachedPeerData.streamType, 'public_server': publicServer,
        'peer_platform': pi.platform, 'peer_version': pi.version,
        'permission_overrides': Map<String, bool>.from(model.permissions),
        'view_only': model.viewOnly},
      'scope': 'peer_preference',
      'effect': 'Settings are persisted and sent immediately; actual codec, FPS and chroma are independent observations. Missing metrics are unknown; values older than 10 seconds are stale except negotiated codec/chroma, retained until reconnection.'
    };
  }

  Future<String> setConnection(String request, Map<String, dynamic> change) async {
    final before = await connectionState();
    final support = before['support'] as Map<String, dynamic>;
    final setting = change['setting'];
    final enabled = change['enabled'] == true;
    guard(request);
    switch (setting) {
      case 'quality':
        if (change['preset'] == 'custom') {
          if ((change['quality'] as int) > support['quality_max']) {
            throw _ViewError('UNSUPPORTED', 'Quality exceeds the stock limit for this peer version or public relay');
          }
          if (change['fps'] != null && support['custom_fps'] != true) {
            throw _ViewError('UNSUPPORTED', 'Custom FPS is disabled by the stock public-relay or peer-version restriction');
          }
          await bind.sessionSetCustomImageQuality(sessionId: ffi.sessionId, value: change['quality']);
          if (change['fps'] != null) {
            guard(request);
            await bind.sessionSetCustomFps(sessionId: ffi.sessionId, fps: change['fps']);
          }
        } else {
          await bind.sessionSetImageQuality(sessionId: ffi.sessionId, value: change['preset']);
        }
        return 'peer_preference';
      case 'codec':
        if (support['codecs_known'] != true) {
          throw _ViewError('CODEC_UNKNOWN', 'Codec negotiation is not ready');
        }
        if (!(support['available_codecs'] as List).contains(change['preference'])) {
          throw _ViewError('UNSUPPORTED', 'Codec is not supported by both decoder and peer encoder');
        }
        await bind.sessionPeerOption(sessionId: ffi.sessionId, name: kOptionCodecPreference, value: change['preference']);
        guard(request);
        await bind.sessionChangePreferCodec(sessionId: ffi.sessionId);
        return 'peer_preference';
      case 'true_color':
        if (enabled && support['true_color'] != true) {
          throw _ViewError('UNSUPPORTED', 'True color requires peer 1.2.4 and observed VP9/AV1');
        }
        await setToggle(request, 'i444', enabled);
        guard(request);
        await bind.sessionChangePreferCodec(sessionId: ffi.sessionId);
        return 'peer_preference';
      case 'audio_muted':
        if (support['audio_permission'] != true) {
          throw _ViewError('PERMISSION_DENIED', 'Remote audio permission is not granted');
        }
        await setToggle(request, 'disable-audio', enabled);
        return 'peer_preference';
      case 'lock_after_end':
        if (support['lock_after_end'] != true) {
          throw _ViewError('PERMISSION_DENIED', 'Lock after session end requires keyboard permission, non-Android peer and view-only off');
        }
        await setToggle(request, 'lock-after-session-end', enabled);
        return 'peer_preference';
      case 'quality_overlay':
        await setToggle(request, 'show-quality-monitor', enabled);
        guard(request);
        await ffi.qualityMonitorModel.checkShowQualityMonitor(ffi.sessionId);
        return 'peer_preference_current_view';
      default:
        throw _ViewError('INVALID_ARGUMENT', 'Unknown connection setting');
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
        case 'connection_get': break;
        case 'connection_set':
          scope = await setConnection(request, command['change']);
          delivery = 'applied';
          break;
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
      final connection = command['command'] == 'connection_get' || command['command'] == 'connection_set';
      final observed = connection ? await connectionState() : await state();
      if (command['command'] == 'set' && command['change']['setting'] == 'fullscreen' &&
          observed['fullscreen'] != command['change']['enabled']) {
        confirmed = false;
        delivery = 'sent';
      }
      result = {'confirmed': confirmed, 'delivery': delivery, 'scope': scope, 'state': observed,
        if (connection && command['command'] == 'connection_set') 'remote_effect_confirmed': false};
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
