//! CLI 层面的可配置重试间隔端到端测试。
//!
//! 验证 `cc-switch proxy config --retry-interval-seconds <N>` 能写入并持久化到
//! proxy_config（per-app），且进程重启（重新打开 DB）后仍生效。

#![allow(clippy::await_holding_lock)]

use serial_test::serial;

use cc_switch_lib::{cli::commands::proxy::ProxyCommand, AppType, Database};

#[path = "support.rs"]
mod support;
use support::{ensure_test_home, lock_test_mutex};

// proxy::execute 内部自建 tokio runtime 并 block_on，因此必须在同步测试里调用，
// 不能放在 #[tokio::test] 之中（否则 "Cannot start a runtime from within a runtime"）。
#[test]
#[serial]
fn proxy_config_retry_interval_persists_across_reopen() {
    let _guard = lock_test_mutex();
    ensure_test_home();

    // 通过 CLI 子命令写入：configure_proxy 走 get_proxy_config_for_app + update_proxy_config_for_app。
    cc_switch_lib::cli::commands::proxy::execute(
        ProxyCommand::Config {
            listen_address: None,
            listen_port: None,
            retry_interval_seconds: Some(9),
        },
        Some(AppType::Codex),
    )
    .expect("proxy config --retry-interval-seconds should succeed");

    // 模拟 daemon 重启：重新打开同一个 DB（CC_SWITCH_CONFIG_DIR 不变），再异步读取验证。
    let rt = tokio::runtime::Runtime::new().expect("create tokio runtime for reads");
    rt.block_on(async {
        let db = Database::init().expect("reopen database after cli write");
        let codex = db
            .get_proxy_config_for_app("codex")
            .await
            .expect("read codex proxy config");
        assert_eq!(
            codex.retry_interval_seconds, 9,
            "CLI setter must persist retry_interval_seconds per app"
        );

        // 其它 app 不受影响（per-app 存储）。
        let claude = db
            .get_proxy_config_for_app("claude")
            .await
            .expect("read claude proxy config");
        assert_eq!(claude.retry_interval_seconds, 0);
    });
}

#[test]
#[serial]
fn proxy_config_retry_interval_rejects_out_of_range() {
    let _guard = lock_test_mutex();
    ensure_test_home();

    let err = cc_switch_lib::cli::commands::proxy::execute(
        ProxyCommand::Config {
            listen_address: None,
            listen_port: None,
            retry_interval_seconds: Some(u32::MAX),
        },
        Some(AppType::Codex),
    )
    .expect_err("out-of-range interval should be rejected");

    let msg = format!("{err}");
    assert!(
        msg.contains("exceeds maximum"),
        "expected range-violation error, got: {msg}"
    );
}
