#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SponsorProviderPreset {
    pub(crate) id: &'static str,
    pub(crate) provider_name: &'static str,
    pub(crate) chip_label: &'static str,
    pub(crate) website_url: &'static str,
    pub(crate) register_url: &'static str,
    pub(crate) promo_code: &'static str,
    pub(crate) partner_promotion_key: &'static str,
    pub(crate) claude_base_url: &'static str,
    pub(crate) codex_base_url: &'static str,
    pub(crate) gemini_base_url: &'static str,
    pub(crate) opencode_base_url: &'static str,
    pub(crate) openclaw_base_url: &'static str,
    pub(crate) hermes_base_url: &'static str,
}

pub(crate) const CLAUDE_API: SponsorProviderPreset = SponsorProviderPreset {
    id: "claudeapi",
    provider_name: "ClaudeAPI",
    chip_label: "* ClaudeAPI",
    website_url: "https://www.apito.ai",
    register_url: "https://console.apito.ai/agent/register/Bsi9NDlWGpkPoAii",
    promo_code: "",
    partner_promotion_key: "claudeapi",
    claude_base_url: "https://gw.apito.ai",
    codex_base_url: "",
    gemini_base_url: "",
    opencode_base_url: "",
    openclaw_base_url: "",
    hermes_base_url: "",
};

pub(crate) const PACKY_CODE: SponsorProviderPreset = SponsorProviderPreset {
    id: "packycode",
    provider_name: "PackyCode",
    chip_label: "* PackyCode",
    website_url: "https://www.packyapi.com",
    register_url: "https://www.packyapi.com/register?aff=cc-switch-cli",
    promo_code: "cc-switch-cli",
    partner_promotion_key: "packycode",
    claude_base_url: "https://www.packyapi.ai",
    codex_base_url: "https://www.packyapi.ai/v1",
    gemini_base_url: "https://www.packyapi.ai",
    opencode_base_url: "https://www.packyapi.ai/v1",
    openclaw_base_url: "https://www.packyapi.ai",
    hermes_base_url: "https://www.packyapi.ai",
};

pub(crate) const CUBENCE: SponsorProviderPreset = SponsorProviderPreset {
    id: "cubence",
    provider_name: "Cubence",
    chip_label: "* Cubence",
    website_url: "https://cubence.com",
    register_url: "https://cubence.com/signup?code=SC3M1CAH&source=ccscli",
    promo_code: "CCSCLI",
    partner_promotion_key: "cubence",
    claude_base_url: "https://api.cubence.com",
    codex_base_url: "https://api.cubence.com/v1",
    gemini_base_url: "https://api.cubence.com",
    opencode_base_url: "https://api.cubence.com/v1",
    openclaw_base_url: "https://api.cubence.com",
    hermes_base_url: "https://api.cubence.com",
};

pub(crate) const OPENMODEL: SponsorProviderPreset = SponsorProviderPreset {
    id: "openmodel",
    provider_name: "OpenModel",
    chip_label: "* OpenModel",
    website_url: "https://www.openmodel.ai/?ref=JGDNqZl8",
    register_url: "https://www.openmodel.ai/?ref=JGDNqZl8",
    promo_code: "",
    partner_promotion_key: "openmodel",
    claude_base_url: "https://api.openmodel.ai",
    codex_base_url: "https://api.openmodel.ai/v1",
    gemini_base_url: "https://api.openmodel.ai",
    opencode_base_url: "https://api.openmodel.ai/v1",
    openclaw_base_url: "https://api.openmodel.ai",
    hermes_base_url: "https://api.openmodel.ai",
};

pub(crate) const RUN_API: SponsorProviderPreset = SponsorProviderPreset {
    id: "runapi",
    provider_name: "RunAPI",
    chip_label: "* RunAPI",
    website_url: "https://runapi.co",
    register_url: "https://runapi.co/register?aff=kTlB",
    promo_code: "",
    partner_promotion_key: "runapi",
    claude_base_url: "https://runapi.co",
    codex_base_url: "https://runapi.co/v1",
    gemini_base_url: "",
    opencode_base_url: "https://runapi.co",
    openclaw_base_url: "https://runapi.co",
    hermes_base_url: "https://runapi.co",
};

pub(crate) const AI_CODE_MIRROR: SponsorProviderPreset = SponsorProviderPreset {
    id: "aicodemirror",
    provider_name: "AICodeMirror",
    chip_label: "* AICodeMirror",
    website_url: "https://www.aicodemirror.ai",
    register_url: "https://www.aicodemirror.ai/register?invitecode=77V9EA",
    promo_code: "",
    partner_promotion_key: "aicodemirror",
    claude_base_url: "https://api.aicodemirror.ai/api/claudecode",
    codex_base_url: "https://api.aicodemirror.ai/api/codex/backend-api/codex",
    gemini_base_url: "https://api.aicodemirror.ai/api/gemini",
    opencode_base_url: "https://api.aicodemirror.ai/api/claudecode",
    openclaw_base_url: "https://api.aicodemirror.ai/api/claudecode",
    hermes_base_url: "https://api.aicodemirror.ai/api/claudecode",
};

pub(crate) const DDS: SponsorProviderPreset = SponsorProviderPreset {
    id: "dds",
    provider_name: "DDS",
    chip_label: "* DDS",
    website_url: "https://www.ddshub.cc",
    register_url: "https://ddshub.short.gy/ccscli",
    promo_code: "",
    partner_promotion_key: "dds",
    claude_base_url: "https://www.ddshub.cc",
    codex_base_url: "https://www.ddshub.cc",
    gemini_base_url: "",
    opencode_base_url: "",
    openclaw_base_url: "",
    hermes_base_url: "",
};

pub(crate) const QINIU: SponsorProviderPreset = SponsorProviderPreset {
    id: "qiniu",
    provider_name: "Qiniu",
    chip_label: "* Qiniu",
    website_url: "https://s.qiniu.com/FVfiEb",
    register_url: "https://s.qiniu.com/FVfiEb",
    promo_code: "",
    partner_promotion_key: "qiniu",
    claude_base_url: "https://api.qnaigc.com",
    codex_base_url: "https://api.qnaigc.com/bypass/openai/v1",
    gemini_base_url: "https://api.qnaigc.com/bypass/vertex",
    opencode_base_url: "https://api.qnaigc.com/v1",
    openclaw_base_url: "https://api.qnaigc.com/v1",
    hermes_base_url: "https://api.qnaigc.com/v1",
};

pub(crate) const FENNO: SponsorProviderPreset = SponsorProviderPreset {
    id: "fenno",
    provider_name: "FennoAI",
    chip_label: "* FennoAI",
    website_url: "https://api.fenno.ai",
    register_url:
        "https://api.fenno.ai/register?redirect=/purchase?tab=subscription%26group=16&aff=Z6XB52KCVP6Y",
    promo_code: "",
    partner_promotion_key: "fenno",
    claude_base_url: "https://api.fenno.ai",
    codex_base_url: "https://api.fenno.ai",
    gemini_base_url: "",
    opencode_base_url: "https://api.fenno.ai/v1",
    openclaw_base_url: "https://api.fenno.ai/v1",
    hermes_base_url: "https://api.fenno.ai/v1",
};

pub(crate) const SPONSOR_PROVIDER_PRESETS: [SponsorProviderPreset; 9] = [
    AI_CODE_MIRROR,
    CLAUDE_API,
    PACKY_CODE,
    CUBENCE,
    OPENMODEL,
    RUN_API,
    DDS,
    QINIU,
    FENNO,
];

const CLAUDE_SPONSOR_PRESETS: [SponsorProviderPreset; 9] = [
    AI_CODE_MIRROR,
    CLAUDE_API,
    CUBENCE,
    OPENMODEL,
    RUN_API,
    QINIU,
    FENNO,
    PACKY_CODE,
    DDS,
];
const CODEX_SPONSOR_PRESETS: [SponsorProviderPreset; 8] = [
    AI_CODE_MIRROR,
    CUBENCE,
    OPENMODEL,
    RUN_API,
    QINIU,
    FENNO,
    PACKY_CODE,
    DDS,
];
const GEMINI_SPONSOR_PRESETS: [SponsorProviderPreset; 5] =
    [AI_CODE_MIRROR, CUBENCE, OPENMODEL, QINIU, PACKY_CODE];
const ADDITIVE_SPONSOR_PRESETS: [SponsorProviderPreset; 7] = [
    AI_CODE_MIRROR,
    CUBENCE,
    OPENMODEL,
    RUN_API,
    QINIU,
    FENNO,
    PACKY_CODE,
];

pub(crate) fn sponsor_provider_preset(id: &str) -> Option<SponsorProviderPreset> {
    SPONSOR_PROVIDER_PRESETS
        .iter()
        .copied()
        .find(|preset| preset.id == id)
}

pub(crate) fn sponsor_provider_presets_for_app(
    app_type: &AppType,
) -> &'static [SponsorProviderPreset] {
    match app_type {
        AppType::Claude => &CLAUDE_SPONSOR_PRESETS,
        AppType::Codex => &CODEX_SPONSOR_PRESETS,
        AppType::Gemini => &GEMINI_SPONSOR_PRESETS,
        AppType::OpenCode | AppType::Hermes | AppType::OpenClaw => &ADDITIVE_SPONSOR_PRESETS,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packycode_keeps_cli_promotion_url_but_uses_upstream_api_hosts() {
        assert_eq!(PACKY_CODE.website_url, "https://www.packyapi.com");
        assert_eq!(
            PACKY_CODE.register_url,
            "https://www.packyapi.com/register?aff=cc-switch-cli"
        );
        assert_eq!(PACKY_CODE.claude_base_url, "https://www.packyapi.ai");
        assert_eq!(PACKY_CODE.codex_base_url, "https://www.packyapi.ai/v1");
        assert_eq!(PACKY_CODE.gemini_base_url, "https://www.packyapi.ai");
        assert_eq!(PACKY_CODE.opencode_base_url, "https://www.packyapi.ai/v1");
        assert_eq!(PACKY_CODE.hermes_base_url, "https://www.packyapi.ai");
        assert_eq!(PACKY_CODE.openclaw_base_url, "https://www.packyapi.ai");
    }

    #[test]
    fn aicodemirror_uses_cli_affiliate_and_upstream_api_hosts() {
        assert_eq!(AI_CODE_MIRROR.website_url, "https://www.aicodemirror.ai");
        assert_eq!(
            AI_CODE_MIRROR.register_url,
            "https://www.aicodemirror.ai/register?invitecode=77V9EA"
        );
        assert_eq!(
            AI_CODE_MIRROR.claude_base_url,
            "https://api.aicodemirror.ai/api/claudecode"
        );
        assert_eq!(
            AI_CODE_MIRROR.codex_base_url,
            "https://api.aicodemirror.ai/api/codex/backend-api/codex"
        );
        assert_eq!(
            AI_CODE_MIRROR.gemini_base_url,
            "https://api.aicodemirror.ai/api/gemini"
        );
        assert_eq!(
            AI_CODE_MIRROR.opencode_base_url,
            "https://api.aicodemirror.ai/api/claudecode"
        );
        assert_eq!(
            AI_CODE_MIRROR.hermes_base_url,
            "https://api.aicodemirror.ai/api/claudecode"
        );
        assert_eq!(
            AI_CODE_MIRROR.openclaw_base_url,
            "https://api.aicodemirror.ai/api/claudecode"
        );
    }

    #[test]
    fn openmodel_uses_affiliate_url_and_protocol_specific_api_hosts() {
        assert_eq!(
            OPENMODEL.website_url,
            "https://www.openmodel.ai/?ref=JGDNqZl8"
        );
        assert_eq!(OPENMODEL.register_url, OPENMODEL.website_url);
        assert_eq!(OPENMODEL.claude_base_url, "https://api.openmodel.ai");
        assert_eq!(OPENMODEL.codex_base_url, "https://api.openmodel.ai/v1");
        assert_eq!(OPENMODEL.gemini_base_url, "https://api.openmodel.ai");
        assert_eq!(OPENMODEL.opencode_base_url, "https://api.openmodel.ai/v1");
        assert_eq!(OPENMODEL.hermes_base_url, "https://api.openmodel.ai");
        assert_eq!(OPENMODEL.openclaw_base_url, "https://api.openmodel.ai");
    }

    #[test]
    fn additive_apps_share_one_sponsor_support_matrix() {
        let expected = [
            "aicodemirror",
            "cubence",
            "openmodel",
            "runapi",
            "qiniu",
            "fenno",
            "packycode",
        ];
        for app_type in [AppType::OpenCode, AppType::Hermes, AppType::OpenClaw] {
            let ids = sponsor_provider_presets_for_app(&app_type)
                .iter()
                .map(|preset| preset.id)
                .collect::<Vec<_>>();
            assert_eq!(ids, expected);
        }
    }

    #[test]
    fn aicodemirror_is_the_first_sponsor_for_every_supported_app() {
        for app_type in [
            AppType::Claude,
            AppType::Codex,
            AppType::Gemini,
            AppType::OpenCode,
            AppType::Hermes,
            AppType::OpenClaw,
        ] {
            assert_eq!(
                sponsor_provider_presets_for_app(&app_type)
                    .first()
                    .map(|preset| preset.id),
                Some("aicodemirror"),
                "AICodeMirror should be the first sponsor preset for {app_type:?}"
            );
        }
    }
}
use crate::app_config::AppType;
