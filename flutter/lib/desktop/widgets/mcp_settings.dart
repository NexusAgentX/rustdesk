import 'dart:async';
import 'dart:convert';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_hbb/common.dart';
import 'package:flutter_hbb/models/platform_model.dart';

class McpSettings extends StatefulWidget {
  const McpSettings({super.key});
  @override
  State<McpSettings> createState() => _McpSettingsState();
}

class _McpSettingsState extends State<McpSettings> {
  Map<String, dynamic> _state = {};
  final _port = TextEditingController();
  Timer? _timer;
  String? _message;
  String _last = '';

  @override
  void initState() {
    super.initState();
    _read();
    _timer = Timer.periodic(const Duration(seconds: 1), (_) => _read());
  }

  void _read() {
    if (!mounted) return;
    final raw = bind.mcpSettings();
    if (raw == _last) return;
    final value = jsonDecode(raw) as Map<String, dynamic>;
    setState(() {
      _last = raw;
      _state = value;
      if (_port.text.isEmpty) {
        _port.text = '${value['status']?['settings']?['port'] ?? 21122}';
      }
    });
  }

  void _configure({bool? enabled, bool? approval}) {
    final port = int.tryParse(_port.text);
    if (port == null || port < 1 || port > 65535) {
      setState(() => _message = translate('Port must be between 1 and 65535'));
      return;
    }
    final settings = _state['status']?['settings'] ?? {};
    final error = bind.mcpConfigure(
        enabled: enabled ?? settings['enabled'] == true,
        port: port,
        approvalRequired: approval ?? settings['approval_required'] != false);
    setState(() => _message = error.isEmpty ? null : error);
    _read();
  }

  Future<void> _copy(String text) async {
    await Clipboard.setData(ClipboardData(text: text));
    if (mounted) setState(() => _message = translate('Copied'));
  }

  Future<void> _credential({bool reset = false, bool config = false}) async {
    final result =
        jsonDecode(bind.mcpCredential(reset: reset)) as Map<String, dynamic>;
    if (result['error'] != null) {
      setState(() => _message = result['error']);
      return;
    }
    if (reset) {
      setState(() =>
          _message = translate('MCP token reset; agents were disconnected'));
      _read();
      return;
    }
    final token = result['token'] as String;
    final address = _state['status']?['address'];
    if (config && address == null) {
      setState(() => _message =
          translate('Start MCP before copying connection configuration'));
      return;
    }
    await _copy(config
        ? const JsonEncoder.withIndent('  ').convert({
            'mcpServers': {
              'rustdesk': {
                'url': address,
                'headers': {'Authorization': 'Bearer $token'}
              }
            }
          })
        : token);
  }

  @override
  void dispose() {
    _timer?.cancel();
    _port.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final status = _state['status'] ?? {};
    final settings = status['settings'] ?? {};
    final address = status['address'];
    final agents = (_state['agents'] as List?) ?? [];
    final names = {
      'disabled': 'Stopped',
      'starting': 'Starting',
      'running': 'Running',
      'failed': 'Failed'
    };
    return ListView(padding: const EdgeInsets.all(24), children: [
      Text('MCP', style: Theme.of(context).textTheme.headlineSmall),
      const SizedBox(height: 12),
      SwitchListTile(
          contentPadding: EdgeInsets.zero,
          title: Text(translate('Enable MCP service')),
          subtitle: Text(translate(names[status['state']] ?? 'Stopped')),
          value: settings['enabled'] == true,
          onChanged: (value) => _configure(enabled: value)),
      if (status['error'] != null)
        SelectableText(status['error'],
            style: TextStyle(color: Theme.of(context).colorScheme.error)),
      Row(children: [
        SizedBox(
            width: 180,
            child: TextField(
                controller: _port,
                keyboardType: TextInputType.number,
                decoration:
                    InputDecoration(labelText: translate('Local HTTP port')),
                onSubmitted: (_) => _configure())),
        const SizedBox(width: 12),
        TextButton(
            onPressed: () => _configure(), child: Text(translate('Apply')))
      ]),
      SwitchListTile(
          contentPadding: EdgeInsets.zero,
          title: Text(translate('Require human approval for AI control')),
          value: settings['approval_required'] != false,
          onChanged: (value) => _configure(approval: value)),
      SwitchListTile(
          contentPadding: EdgeInsets.zero,
          title: Text(translate('Follow AI display')),
          value: bind.mainGetLocalOption(key: 'mcp-follow-display') == 'Y',
          onChanged: (value) {
            bind.mainSetLocalOption(key: 'mcp-follow-display', value: value ? 'Y' : 'N');
            setState(() {});
          }),
      if (address != null)
        Row(children: [
          Expanded(child: SelectableText(address)),
          IconButton(
              tooltip: translate('Copy'),
              onPressed: () => _copy(address),
              icon: const Icon(Icons.copy))
        ]),
      Wrap(spacing: 8, children: [
        OutlinedButton(
            onPressed: () => _credential(),
            child: Text(translate('Copy MCP token'))),
        OutlinedButton(
            onPressed: () => _credential(reset: true),
            child: Text(translate('Reset MCP token'))),
        OutlinedButton(
            onPressed: () => _credential(config: true),
            child: Text(translate('Copy AI connection configuration'))),
      ]),
      if (_message != null)
        Padding(
            padding: const EdgeInsets.only(top: 8),
            child: SelectableText(_message!)),
      const Divider(height: 32),
      Text(translate('Connected agents'),
          style: Theme.of(context).textTheme.titleMedium),
      const SizedBox(height: 8),
      if (agents.isEmpty) Text(translate('No agents connected')),
      for (final agent in agents)
        Card(
            child: Padding(
                padding: const EdgeInsets.all(12),
                child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Text('${agent['name']} ${agent['version']}',
                          style: Theme.of(context).textTheme.titleSmall),
                      SelectableText(agent['agent_id']),
                      Text(
                          '${translate('Connected at')}: ${agent['connected_at']}'),
                      Text(
                          '${translate('Last communication')}: ${agent['last_communication']}'),
                      for (final session in agent['sessions'])
                        Padding(
                            padding: const EdgeInsets.only(top: 8),
                            child: Column(
                                crossAxisAlignment: CrossAxisAlignment.start,
                                children: [
                                  Text(
                                      '${session['peer_id']} · ${session['connection_state']} · ${session['control']}'),
                                  SelectableText(session['session_id']),
                                  for (final view in session['ui_session_ids'])
                                    SelectableText('GUI: $view'),
                                  for (final terminal in session['terminals'])
                                    SelectableText(
                                        '${terminal['terminal_id']} · ${terminal['state']}'),
                                ])),
                    ]))),
    ]);
  }
}
