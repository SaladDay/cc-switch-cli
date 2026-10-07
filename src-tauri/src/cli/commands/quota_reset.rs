//! Shared validation and output for the two explicit reset-credit commands.
use crate::cli::i18n::is_chinese;
use crate::cli::ui::{highlight, warning};
use crate::error::AppError;
use crate::services::{CodexResetCredit, CredentialStatus, SubscriptionQuota};

pub(super) fn quota_error(quota: &SubscriptionQuota) -> Option<String> {
    if quota.success && quota.credential_status == CredentialStatus::Valid && quota.error.is_none()
    {
        return None;
    }
    Some(
        quota
            .error
            .as_deref()
            .or(quota.credential_message.as_deref())
            .map(str::to_owned)
            .unwrap_or_else(|| {
                if is_chinese() {
                    format!("额度查询失败（凭据状态：{:?}）", quota.credential_status)
                } else {
                    format!(
                        "Quota query failed (credential status: {:?})",
                        quota.credential_status
                    )
                }
            }),
    )
}

pub(super) fn select_credit(
    quota: &SubscriptionQuota,
    credit_id: Option<&str>,
) -> Result<CodexResetCredit, AppError> {
    if let Some(error) = quota_error(quota) {
        return Err(AppError::Message(error));
    }
    let summary = quota.reset_credits.as_ref().ok_or_else(|| {
        AppError::localized(
            "quota.reset.inspection_missing",
            "重置卡查询结果缺失，请重试",
            "Reset credit inspection is unavailable; retry the query",
        )
    })?;
    if summary.inspection_error.is_some()
        || (summary.available_count > 0 && summary.credits.is_empty())
    {
        return Err(AppError::localized(
            "quota.reset.inspection_failed",
            "重置卡查询失败或不完整，请重试",
            "Reset credit inspection failed or is incomplete; retry the query",
        ));
    }
    let credits = &summary.credits;
    match credit_id {
        Some(id) => credits.iter().find(|c| c.id == id),
        None => credits.first(),
    }
    .cloned()
    .ok_or_else(|| {
        AppError::localized(
            "quota.reset.credit_unavailable",
            "没有可用的目标重置卡，请重新查询额度",
            "No matching reset credit is available. Query the quota again.",
        )
    })
}

pub(super) fn auth_confirmation(account_id: &str, credit_id: &str) -> Result<String, AppError> {
    confirmation_command(&[
        "cc-switch",
        "auth",
        "reset-quota",
        "--account-id",
        account_id,
        "--credit-id",
        credit_id,
        "--confirm",
    ])
}

pub(super) fn provider_confirmation(
    app: &str,
    provider_id: &str,
    credit_id: &str,
    account_id: Option<&str>,
) -> Result<String, AppError> {
    // Managed OAuth confirmation uses the account entry point to avoid re-resolving
    // mutable provider/default-account bindings in a later invocation.
    if let Some(account_id) = account_id {
        return auth_confirmation(account_id, credit_id);
    }
    confirmation_command(&[
        "cc-switch",
        "--app",
        app,
        "provider",
        "quota",
        provider_id,
        "--reset",
        "--credit-id",
        credit_id,
        "--confirm",
    ])
}

pub(super) fn confirmation_command(args: &[&str]) -> Result<String, AppError> {
    args.iter()
        .map(|arg| {
            shlex::try_quote(arg).map(|q| q.into_owned()).map_err(|_| {
                AppError::localized(
                    "quota.reset.invalid_argument",
                    "重置参数包含无效字符",
                    "Invalid character in reset argument",
                )
            })
        })
        .collect::<Result<Vec<_>, _>>()
        .map(|args| args.join(" "))
}

pub(super) fn print_updated_quota(quota: Option<&SubscriptionQuota>) {
    let Some(quota) = quota else {
        return;
    };
    if let Some(error) = quota_error(quota) {
        let message = if is_chinese() {
            format!("重置卡已兑换，但额度复查失败：{error}。请勿重复兑换。")
        } else {
            format!(
                "Credit redeemed, but quota re-inspection failed: {error}. Do not redeem again."
            )
        };
        println!("{}", warning(&message));
        return;
    }
    println!();
    println!(
        "{}",
        highlight(if is_chinese() {
            "更新后的额度："
        } else {
            "Updated Quota:"
        })
    );
    for tier in &quota.tiers {
        println!("  {}: {:.1}%", tier.name, tier.utilization);
    }
    if let Some(rc) = &quota.reset_credits {
        if rc.inspection_error.is_some() {
            println!(
                "{}",
                warning(if is_chinese() {
                    "额度已更新，但剩余重置卡查询失败，请重试查询。"
                } else {
                    "Quota updated, but remaining reset credits could not be inspected; retry the query."
                })
            );
            return;
        }
        println!(
            "  {}: {}",
            if is_chinese() {
                "剩余重置卡"
            } else {
                "Remaining Credits"
            },
            rc.available_count
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn failed_inspection_is_not_reported_as_zero_credits() {
        let quota =
            SubscriptionQuota::error("codex", CredentialStatus::Expired, "token rejected".into());
        assert!(select_credit(&quota, None)
            .unwrap_err()
            .to_string()
            .contains("token rejected"));
        assert_eq!(quota_error(&quota).as_deref(), Some("token rejected"));
        assert!(quota_error(&SubscriptionQuota::not_found("codex")).is_some());
    }
    #[test]
    fn confirmation_preserves_target_and_quotes_shell_metacharacters() {
        let args = [
            "cc-switch",
            "auth",
            "reset-quota",
            "--account-id",
            "account B",
            "--credit-id",
            "card'$(touch /tmp/nope)",
            "--confirm",
        ];
        let command = auth_confirmation("account B", "card'$(touch /tmp/nope)").unwrap();
        assert_eq!(shlex::split(&command).unwrap(), args);
        assert!(confirmation_command(&["bad\0arg"]).is_err());
    }
    #[test]
    fn provider_confirmation_keeps_app_provider_and_selected_credit() {
        let command =
            provider_confirmation("codex", "provider b", "specific-credit", None).unwrap();
        assert_eq!(
            shlex::split(&command).unwrap(),
            [
                "cc-switch",
                "--app",
                "codex",
                "provider",
                "quota",
                "provider b",
                "--reset",
                "--credit-id",
                "specific-credit",
                "--confirm"
            ]
        );
    }
    #[test]
    fn managed_provider_confirmation_pins_the_resolved_account() {
        let command = provider_confirmation(
            "codex",
            "provider",
            "specific-credit",
            Some("resolved-account"),
        )
        .unwrap();
        assert_eq!(
            shlex::split(&command).unwrap(),
            [
                "cc-switch",
                "auth",
                "reset-quota",
                "--account-id",
                "resolved-account",
                "--credit-id",
                "specific-credit",
                "--confirm"
            ]
        );
    }

    #[test]
    fn missing_or_incomplete_credit_inspection_is_not_an_empty_result() {
        let mut quota: SubscriptionQuota = serde_json::from_value(serde_json::json!({
            "tool":"codex", "credentialStatus":"valid", "success":true, "tiers":[]
        }))
        .unwrap();
        assert!(matches!(
            select_credit(&quota, None).unwrap_err(),
            AppError::Localized {
                key: "quota.reset.inspection_missing",
                ..
            }
        ));
        quota.reset_credits = Some(crate::services::CodexResetCreditsSummary {
            available_count: 1,
            applicable_available_count: None,
            credits: vec![],
            inspection_error: None,
        });
        assert!(matches!(
            select_credit(&quota, None).unwrap_err(),
            AppError::Localized {
                key: "quota.reset.inspection_failed",
                ..
            }
        ));
        quota.reset_credits.as_mut().unwrap().available_count = 0;
        assert!(matches!(
            select_credit(&quota, None).unwrap_err(),
            AppError::Localized {
                key: "quota.reset.credit_unavailable",
                ..
            }
        ));
        quota.reset_credits.as_mut().unwrap().inspection_error = Some("failed".into());
        assert!(matches!(
            select_credit(&quota, None).unwrap_err(),
            AppError::Localized {
                key: "quota.reset.inspection_failed",
                ..
            }
        ));
    }

    #[test]
    fn selection_preserves_specific_credit_and_refuses_missing_credit() {
        let quota: SubscriptionQuota = serde_json::from_value(serde_json::json!({
            "tool":"codex", "credentialStatus":"valid", "success":true, "tiers":[],
            "resetCredits":{"availableCount":2,"credits":[
                {"id":"first"},{"id":"selected"}
            ]}
        }))
        .unwrap();
        assert_eq!(
            select_credit(&quota, Some("selected")).unwrap().id,
            "selected"
        );
        assert!(select_credit(&quota, Some("missing")).is_err());
        assert!(quota_error(&quota).is_none());
    }
}
