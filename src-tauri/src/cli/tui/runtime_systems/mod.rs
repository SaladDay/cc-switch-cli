mod handlers;
mod types;
mod workers;

#[cfg(test)]
pub(crate) use handlers::{apply_webdav_jianguoyun_quick_setup, update_webdav_last_error_with};
pub(crate) use handlers::{
    handle_local_env_msg, handle_managed_auth_msg, handle_model_fetch_msg, handle_proxy_msg,
    handle_quota_msg, handle_session_msg, handle_skills_msg, handle_speedtest_msg,
    handle_stream_check_msg, handle_update_msg, handle_webdav_msg,
};
#[cfg(test)]
pub(crate) use types::{
    build_model_fetch_candidate_urls, model_fetch_strategy_for_field,
    parse_model_ids_from_response, ManagedAuthMsg, ProxyMsg, UpdateMsg,
};
pub(crate) use types::{
    build_stream_check_result_lines, fetch_provider_models_for_tui, ModelFetchStrategy,
};
pub(crate) use types::{
    next_model_fetch_request_id, AppDataLoadKind, AppDataMsg, AppDataReq, LocalEnvReq,
    ManagedAuthReq, ModelFetchReq, ProxyReq, QuotaReq, RequestTracker, SessionReq, SkillsReq,
    StreamCheckReq, UpdateReq, UsageLogLoadError, UsagePricingLoadError, UsagePricingMsg,
    UsagePricingReq, WebDavReq, WebDavReqKind,
};
pub(crate) use types::{SessionUsageSyncMsg, SessionUsageSyncReq};
#[cfg(test)]
pub(crate) use workers::drain_latest_webdav_req;
pub(crate) use workers::{
    start_app_data_system, start_local_env_system, start_managed_auth_system,
    start_model_fetch_system, start_proxy_system, start_quota_system, start_session_system,
    start_session_usage_sync_system, start_skills_system, start_speedtest_system,
    start_stream_check_system, start_update_system, start_usage_pricing_system,
    start_webdav_system,
};
