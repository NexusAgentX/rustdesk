// Isolated validation snapshot only. Never copied into the product branch.
static VALIDATION_STARTED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static VALIDATION_DROPPED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
struct ValidationGuard;
impl Drop for ValidationGuard {
    fn drop(&mut self) {
        VALIDATION_DROPPED.fetch_add(1, Ordering::SeqCst);
    }
}
struct ValidationHandler;
impl super::ToolHandler for ValidationHandler {
    fn call(&self, _: super::CallContext, args: serde_json::Map<String, serde_json::Value>) -> super::ToolFuture {
        Box::pin(async move {
            if args.get("action").and_then(|v| v.as_str()) == Some("pending") {
                let _guard = ValidationGuard;
                VALIDATION_STARTED.fetch_add(1, Ordering::SeqCst);
                return std::future::pending().await;
            }
            Ok(super::ToolResult::text(serde_json::json!({
                "fixture": true,
                "started": VALIDATION_STARTED.load(Ordering::SeqCst),
                "dropped": VALIDATION_DROPPED.load(Ordering::SeqCst),
                "args": args
            }).to_string()))
        })
    }
}
fn enable_validation_fixture(service: &Service) {
    if option_env!("MCP_MOBILE_VALIDATION") != Some("enabled") { return; }
    let mut config = ServerConfig::local(super::Implementation {
        name: "rustdesk-mobile-lifecycle-fixture".into(), version: "0-test-only".into()
    });
    config.address.set_port(37173);
    let credentials = Credentials::new(1);
    credentials.insert("mobile-validation".into(), "mobile-validation-fixture-not-a-production-credential-20260918").unwrap();
    service.enable(config, credentials, vec![RegisteredTool {
        definition: super::ToolDefinition {
            name: "mobile_validation_fixture".into(),
            description: "Protocol/lifecycle fixture only; no remote control capability".into(),
            input_schema: serde_json::json!({"type":"object","properties":{"action":{"type":"string"}}}),
            output_schema: None, annotations: None,
        },
        handler: Arc::new(ValidationHandler),
    }]);
}
