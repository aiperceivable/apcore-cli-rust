// apcore-cli — End-to-end verification that an ACL-sourced approval requirement
// reaches `CliApprovalHandler` (apcore >= 0.28.0, PROTOCOL_SPEC §6.1.6-§6.1.8,
// §6.9 rows 3-5, apcore#108).
//
// Before spec v1.28.0 the Step-5 approval gate fired only when the module's own
// `annotations.requires_approval` was true, so the CLI's handler only ever saw
// annotation-gated modules. The gate now fires on the **union** of the
// annotation, an ACL rule carrying `approval: required`, and `gate_destructive`
// — which means a module annotated `requires_approval: false` can now be routed
// through `CliApprovalHandler`.
//
// That matters here because the handler's adapter rebuilds a `module_def` from
// `request.annotations.requires_approval` and returns early when it is not
// `true`. The gate rewrites the annotation to the *effective* value before
// building the request, so the early return must not fire for an ACL-sourced
// requirement — this test measures that rather than inferring it from the code
// path. (apcore-cli-python shipped a handler that failed the equivalent check.)

use apcore::acl::{ACLRule, ApprovalRequirement, ACL};
use apcore::module::Module;
use apcore::{Config, Executor, ModuleAnnotations, Registry};
use apcore_cli::CliApprovalHandler;
use serde_json::{json, Value};
use std::sync::Arc;

/// A stand-in for `git push`: annotated `requires_approval: false`, so only an
/// ACL rule can put one of its calls to a human.
struct GitPush;

#[async_trait::async_trait]
impl Module for GitPush {
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "remote": {"type": "string"},
                "force": {"type": "boolean"}
            }
        })
    }

    fn output_schema(&self) -> Value {
        json!({"type": "object"})
    }

    fn description(&self) -> &str {
        "Push to a remote"
    }

    fn annotations(&self) -> ModuleAnnotations {
        ModuleAnnotations {
            requires_approval: false,
            ..Default::default()
        }
    }

    async fn execute(
        &self,
        inputs: Value,
        _ctx: &apcore::Context<Value>,
    ) -> Result<Value, apcore::errors::ModuleError> {
        let force = inputs
            .get("force")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        Ok(json!({"pushed": true, "force": force}))
    }
}

/// Executor holding `git.push`, an argument-scoped approval rule ahead of a
/// broad allow, and the CLI's own approval handler.
///
/// `auto_approve` selects the handler's disposition: `true` answers "approved",
/// `false` falls through to the TTY prompt, which under `cargo test` has no
/// terminal and therefore rejects. The rejecting variant is what makes these
/// tests discriminating — a passing call proves nothing on its own, since a
/// gate that never fired would also let the call through.
fn executor_with_argument_scoped_rule(auto_approve: bool) -> Executor {
    let registry = Registry::new();
    registry
        .register_module("git.push", Box::new(GitPush))
        .expect("git.push registers");

    let mut executor = Executor::new(Arc::new(registry), Config::default());

    // ACLRule deliberately implements no `Default` (apcore#108): it is a
    // config-shaped struct deployments construct directly, so every field is
    // spelled out here.
    let narrow = ACLRule {
        callers: vec!["*".to_string()],
        targets: vec!["git.push".to_string()],
        effect: "allow".to_string(),
        approval: Some(ApprovalRequirement::Required),
        description: None,
        conditions: Some(json!({"arguments": {"has_key": ["force"]}})),
    };
    let broad = ACLRule {
        callers: vec!["*".to_string()],
        targets: vec!["*".to_string()],
        effect: "allow".to_string(),
        approval: None,
        description: None,
        conditions: None,
    };

    let acl = ACL::try_new(vec![narrow, broad], "deny", None).expect("ACL is well-formed");
    executor.set_acl(acl);
    executor.set_approval_handler(Box::new(CliApprovalHandler::new(auto_approve, 60)));
    executor
}

#[tokio::test]
async fn ungated_call_runs_without_reaching_the_handler() {
    let executor = executor_with_argument_scoped_rule(/*auto_approve*/ true);

    let result = executor
        .call("git.push", json!({"remote": "origin"}), None, None)
        .await
        .expect("a plain push carries no `force` key, so the rule does not match");

    assert_eq!(result, json!({"pushed": true, "force": false}));
}

#[tokio::test]
async fn acl_sourced_approval_reaches_the_cli_handler_and_the_call_runs() {
    let executor = executor_with_argument_scoped_rule(/*auto_approve*/ true);

    // `force` is present, so the narrow rule matches and carries
    // `approval: required` even though the module annotation says false.
    // The gate must invoke CliApprovalHandler rather than skipping, and the
    // handler's auto-approve answer must be accepted as an ApprovalResult.
    let result = executor
        .call(
            "git.push",
            json!({"remote": "origin", "force": true}),
            None,
            None,
        )
        .await
        .expect("the ACL requires a human; the CLI handler answers in auto-approve mode");

    assert_eq!(result, json!({"pushed": true, "force": true}));
}

#[tokio::test]
async fn preflight_reports_the_governance_effective_requirement() {
    // §7.9.5: `validate()` reports the union, which `apcli validate` and the
    // `--dry-run` path forward verbatim as `requires_approval`.
    let executor = executor_with_argument_scoped_rule(/*auto_approve*/ true);

    let plain = executor
        .validate("git.push", &json!({"remote": "origin"}), None)
        .await
        .expect("preflight succeeds");
    let forced = executor
        .validate(
            "git.push",
            &json!({"remote": "origin", "force": true}),
            None,
        )
        .await
        .expect("preflight succeeds");

    assert!(
        !plain.requires_approval,
        "a push with no `force` needs no human"
    );
    assert!(
        forced.requires_approval,
        "the ACL requires a human for a push carrying `force`, even though the \
         module annotation says requires_approval: false"
    );
}

#[tokio::test]
async fn a_refusing_handler_blocks_only_the_acl_matched_call() {
    // The discriminating pair. With auto-approve off and no TTY under `cargo
    // test`, CliApprovalHandler answers "rejected". If the gate did not fire —
    // or fired but never reached this handler — both calls would succeed and
    // this test would not be able to tell the difference.
    let executor = executor_with_argument_scoped_rule(/*auto_approve*/ false);

    let plain = executor
        .call("git.push", json!({"remote": "origin"}), None, None)
        .await;
    assert!(
        plain.is_ok(),
        "a push with no `force` does not match the approval rule, so the \
         refusing handler must never be consulted for it: {plain:?}"
    );

    let forced = executor
        .call(
            "git.push",
            json!({"remote": "origin", "force": true}),
            None,
            None,
        )
        .await;
    let err = forced.expect_err(
        "the ACL requires a human for a push carrying `force`, and the handler refused",
    );
    assert_eq!(
        err.code,
        apcore::errors::ErrorCode::ApprovalDenied,
        "the refusal must surface as APPROVAL_DENIED (exit 46), not as a \
         generic execute error: {err:?}"
    );
}
