import 'dart:async';
import 'dart:convert';
import 'package:flutter/material.dart';
import 'package:flutter_hbb/common.dart';
import 'package:flutter_hbb/models/platform_model.dart';
import 'package:flutter_hbb/models/model.dart';
import 'package:flutter_hbb/models/state_model.dart';
import 'package:desktop_multi_window/desktop_multi_window.dart';

class AutomationSessionView extends StatefulWidget {
  const AutomationSessionView(
      {super.key,
      required this.ffi,
      required this.child,
      this.onHumanControl,
      this.isActive,
      this.containDialogs = false});
  final FFI ffi;
  final Widget child;
  final VoidCallback? onHumanControl;
  final bool Function()? isActive;
  final bool containDialogs;

  @override
  State<AutomationSessionView> createState() => _AutomationSessionViewState();
}

class _AutomationSessionViewState extends State<AutomationSessionView> {
  Timer? _timer;
  Map<String, dynamic> _control = {};
  List<dynamic> _tunnels = [];
  String _last = '';
  String? _error;
  bool _readingWindow = false;
  bool _available = false;
  final _dialogs = OverlayKeyState();
  late final OverlayEntry _content = OverlayEntry(builder: (_) => widget.child);

  @override
  void initState() {
    super.initState();
    _available = jsonDecode(bind.mcpSettings())['available'] == true;
    if (!_available) return;
    _read();
    _timer = Timer.periodic(const Duration(milliseconds: 250), (_) => _read());
  }

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    if (_available && widget.containDialogs && _isActive) {
      widget.ffi.dialogManager.setOverlayState(_dialogs);
    }
  }

  bool get _isActive => widget.isActive?.call() ?? TickerMode.of(context);

  @override
  void didUpdateWidget(covariant AutomationSessionView oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (_available && widget.containDialogs) _content.markNeedsBuild();
  }

  Future<void> _readWindow() async {
    if (_readingWindow || !mounted) return;
    _readingWindow = true;
    try {
      final window = WindowController.fromWindowId(stateGlobal.windowId);
      final visible = !await window.isHidden();
      final minimized = await window.isMinimized();
      if (mounted) {
        await bind.automationGuiVisibility(
            sessionId: widget.ffi.sessionId,
            visible: visible && _isActive,
            minimized: minimized);
      }
    } finally {
      _readingWindow = false;
    }
  }

  void _read() {
    if (!mounted) return;
    if (widget.containDialogs && _isActive) {
      widget.ffi.dialogManager.setOverlayState(_dialogs);
    }
    _readWindow();
    final raw = bind.automationControlState(sessionId: widget.ffi.sessionId);
    if (raw == _last) return;
    final value = jsonDecode(raw) as Map<String, dynamic>;
    final wasReadonly =
        _control['mode'] == 'ai' || _control['transitioning'] == true;
    setState(() {
      _last = raw;
      _control = Map<String, dynamic>.from(value['control'] ?? {});
      _tunnels = List<dynamic>.from(value['tunnels'] ?? []);
    });
    if (wasReadonly &&
        _control['mode'] == 'human' &&
        _control['transitioning'] != true) {
      WidgetsBinding.instance.addPostFrameCallback((_) {
        if (mounted) widget.onHumanControl?.call();
      });
    }
  }

  void _action(String action, [String approvalId = '']) {
    final error = bind.automationControlAction(
        sessionId: widget.ffi.sessionId,
        action: action,
        approvalId: approvalId);
    setState(() => _error = error.isEmpty ? null : error);
    _read();
  }

  @override
  void dispose() {
    _timer?.cancel();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final child = _available && widget.containDialogs
        ? Overlay(key: _dialogs.key, initialEntries: [_content])
        : widget.child;
    final agent = _control['agent_id'];
    final changing = _control['transitioning'] == true;
    if (agent == null && !changing && _control['release_error'] == null) {
      return child;
    }
    final readonly = _control['mode'] == 'ai' || changing;
    final approval = _control['approval'];
    final awaiting = approval is Map && approval['state'] == 'pending';
    return Column(children: [
      Material(
        color: readonly
            ? Theme.of(context).colorScheme.secondaryContainer
            : Theme.of(context).colorScheme.surfaceContainerHighest,
        child: Padding(
            padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 6),
            child:
                Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
              Row(children: [
                Icon(readonly ? Icons.smart_toy_outlined : Icons.person_outline,
                    size: 20),
                const SizedBox(width: 8),
                Expanded(
                    child: Text(
                        '${translate(changing ? 'Changing session control' : readonly ? 'AI controls this session' : 'AI joined; you control this session')} · ${agent ?? ''}',
                        maxLines: 2,
                        overflow: TextOverflow.ellipsis)),
                if (readonly)
                  FilledButton(
                      style: FilledButton.styleFrom(
                          backgroundColor:
                              Theme.of(context).colorScheme.surface,
                          foregroundColor:
                              Theme.of(context).colorScheme.onSurface),
                      onPressed: () => _action('takeover'),
                      child: Text(translate('Take control'))),
                if (awaiting) ...[
                  TextButton(
                      onPressed: () => _action('approve', approval['id']),
                      child: Text(translate('Approve AI control'))),
                  TextButton(
                      onPressed: () => _action('reject', approval['id']),
                      child: Text(translate('Decline'))),
                ],
              ]),
              if (_tunnels.isNotEmpty)
                ConstrainedBox(
                    constraints: const BoxConstraints(maxHeight: 160),
                    child: SingleChildScrollView(
                        child: Column(
                            crossAxisAlignment: CrossAxisAlignment.start,
                            children: [
                              ..._tunnels.where((t) => t['state'] != 'closed' && t['state'] != 'failed'),
                              ..._tunnels.reversed.where((t) => t['state'] == 'closed' || t['state'] == 'failed'),
                            ].take(16).map((t) => Text(
                                "127.0.0.1:${t['local_port']} → ${t['remote_host']}:${t['remote_port']} · ${t['state']} · ${t['active_connections']} connections${t['last_error'] == null ? '' : ' · ${t['last_error']}'}",
                                maxLines: 2, overflow: TextOverflow.ellipsis)).toList()))),
              if (awaiting && (approval['reason'] as String).isNotEmpty)
                Text(approval['reason']),
              if (_error != null || _control['release_error'] != null)
                Text(_error ?? _control['release_error'],
                    style:
                        TextStyle(color: Theme.of(context).colorScheme.error)),
            ])),
      ),
      Expanded(
          child: ExcludeFocus(
              excluding: readonly,
              child: MouseRegion(
                  cursor: readonly
                      ? SystemMouseCursors.forbidden
                      : MouseCursor.defer,
                  child: AbsorbPointer(absorbing: readonly, child: child)))),
    ]);
  }
}
