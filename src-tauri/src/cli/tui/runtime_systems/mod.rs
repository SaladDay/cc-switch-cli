mod handlers;
mod types;
mod workers;

#[cfg(test)]
pub(crate) use handlers::{apply_webdav_jianguoyun_quick_setup, update_webdav_last_error_with};
pub(crate) use handlers::{
    handle_local_env_msg, handle_model_fetch_msg, handle_proxy_msg, handle_quota_msg,
    handle_skills_msg, handle_speedtest_msg, handle_stream_check_msg, handle_update_msg,
    handle_webdav_msg,
};
#[cfg(test)]
pub(crate) use types::{
    build_model_fetch_candidate_urls, model_fetch_strategy_for_field,
    parse_model_ids_from_response, UpdateMsg,
};
pub(crate) use types::{
    build_stream_check_result_lines, fetch_provider_models_for_tui, ModelFetchStrategy,
};
pub(crate) use types::{
    next_model_fetch_request_id, LocalEnvReq, ModelFetchReq, ProxyReq, QuotaReq, RequestTracker,
    SkillsReq, StreamCheckReq, UpdateReq, WebDavReq, WebDavReqKind,
};
#[cfg(test)]
pub(crate) use workers::drain_latest_webdav_req;
pub(crate) use workers::{
    start_local_env_system, start_model_fetch_system, start_proxy_system, start_quota_system,
    start_skills_system, start_speedtest_system, start_stream_check_system, start_update_system,
    start_webdav_system,
};
