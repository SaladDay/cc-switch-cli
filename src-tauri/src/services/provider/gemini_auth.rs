use crate::app_config::AppType;
use crate::error::AppError;
use crate::provider::Provider;
use crate::settings;

use super::ProviderService;

/// Gemini 认证类型枚举
///
/// 区分 OAuth 和 API Key 两种认证方式
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum GeminiAuthType {
    /// Google 官方（使用 OAuth 认证）
    GoogleOfficial,
    /// API Key 认证（包括所有第三方供应商：PackyCode、Generic 等）
    ApiKey,
}

impl ProviderService {
    // 认证类型常量
    const API_KEY_SECURITY_SELECTED_TYPE: &'static str = "gemini-api-key";
    const GOOGLE_OAUTH_SECURITY_SELECTED_TYPE: &'static str = "oauth-personal";

    // Partner Promotion Key 常量
    const GOOGLE_OFFICIAL_PARTNER_KEY: &'static str = "google-official";

    /// 检测 Gemini 供应商的认证类型
    ///
    /// 只区分两种认证方式：OAuth (Google 官方) 和 API Key (所有其他供应商)
    ///
    /// # 返回值
    ///
    /// - `GeminiAuthType::GoogleOfficial`: Google 官方，使用 OAuth
    /// - `GeminiAuthType::ApiKey`: 其他所有供应商，使用 API Key
    pub(super) fn detect_gemini_auth_type(provider: &Provider) -> GeminiAuthType {
        // 检查 partner_promotion_key 是否为 google-official
        if let Some(key) = provider
            .meta
            .as_ref()
            .and_then(|meta| meta.partner_promotion_key.as_deref())
        {
            if key.eq_ignore_ascii_case(Self::GOOGLE_OFFICIAL_PARTNER_KEY) {
                return GeminiAuthType::GoogleOfficial;
            }
        }

        // 检查名称是否为 Google
        let name_lower = provider.name.to_ascii_lowercase();
        if name_lower == "google" || name_lower.starts_with("google ") {
            return GeminiAuthType::GoogleOfficial;
        }

        // 其他所有情况：API Key 认证
        GeminiAuthType::ApiKey
    }

    pub(super) fn gemini_security_selected_type(auth_type: GeminiAuthType) -> &'static str {
        match auth_type {
            GeminiAuthType::GoogleOfficial => Self::GOOGLE_OAUTH_SECURITY_SELECTED_TYPE,
            GeminiAuthType::ApiKey => Self::API_KEY_SECURITY_SELECTED_TYPE,
        }
    }

    pub(super) fn ensure_gemini_app_security_flag(
        auth_type: GeminiAuthType,
    ) -> Result<(), AppError> {
        settings::ensure_security_auth_selected_type(Self::gemini_security_selected_type(auth_type))
    }

    ///
    /// Google 官方 Gemini 使用 OAuth 个人认证，不需要 API Key。
    ///
    /// # 写入两处 settings.json 的原因
    ///
    /// 1. **`~/.cc-switch/settings.json`** (应用级配置):
    ///    - CC-Switch 应用的全局设置
    ///    - 确保应用知道当前使用的认证类型
    ///    - 用于 UI 显示和其他应用逻辑
    ///
    /// 2. **`~/.gemini/settings.json`** (Gemini 客户端配置):
    ///    - Gemini CLI 客户端读取的配置文件
    ///    - 直接影响 Gemini 客户端的认证行为
    ///    - 确保 Gemini 使用正确的认证方式连接 API
    ///
    /// # 设置的值
    ///
    /// ```json
    /// {
    ///   "security": {
    ///     "auth": {
    ///       "selectedType": "oauth-personal"
    ///     }
    ///   }
    /// }
    /// ```
    ///
    /// # OAuth 认证流程
    ///
    /// 1. 用户切换到 Google 官方供应商
    /// 2. CC-Switch 设置 `selectedType = "oauth-personal"`
    /// 3. 用户首次使用 Gemini CLI 时，会自动打开浏览器进行 OAuth 登录
    /// 4. 登录成功后，凭证保存在 Gemini 的 credential store 中
    /// 5. 后续请求自动使用保存的凭证
    #[expect(
        dead_code,
        reason = "kept for explicit Gemini OAuth security flag updates"
    )]
    pub(crate) fn ensure_google_oauth_security_flag(provider: &Provider) -> Result<(), AppError> {
        // 检测是否为 Google 官方
        let auth_type = Self::detect_gemini_auth_type(provider);
        if auth_type != GeminiAuthType::GoogleOfficial {
            return Ok(());
        }

        // 写入应用级别的 settings.json (~/.cc-switch/settings.json)
        settings::ensure_security_auth_selected_type(Self::GOOGLE_OAUTH_SECURITY_SELECTED_TYPE)?;

        // 写入 Gemini 目录的 settings.json (~/.gemini/settings.json)
        if crate::sync_policy::should_sync_live(&AppType::Gemini) {
            use crate::gemini_config::write_google_oauth_settings;
            write_google_oauth_settings()?;
        }

        Ok(())
    }

    /// 确保 API Key 供应商的安全标志正确设置
    ///
    /// 此函数适用于所有使用 API Key 认证的 Gemini 供应商，包括：
    /// - PackyCode（合作伙伴）
    /// - 其他第三方 Gemini API 服务
    ///
    /// 所有 API Key 供应商使用相同的认证方式和配置逻辑。
    ///
    /// # 设置的值
    ///
    /// ```json
    /// {
    ///   "security": {
    ///     "auth": {
    ///       "selectedType": "gemini-api-key"
    ///     }
    ///   }
    /// }
    /// ```
    #[expect(
        dead_code,
        reason = "kept for explicit Gemini API-key security flag updates"
    )]
    pub(crate) fn ensure_api_key_security_flag(_provider: &Provider) -> Result<(), AppError> {
        // 写入应用级别的 settings.json (~/.cc-switch/settings.json)
        settings::ensure_security_auth_selected_type(Self::API_KEY_SECURITY_SELECTED_TYPE)?;

        // 写入 Gemini 目录的 settings.json (~/.gemini/settings.json)
        if crate::sync_policy::should_sync_live(&AppType::Gemini) {
            use crate::gemini_config::write_generic_settings;
            write_generic_settings()?;
        }

        Ok(())
    }
}
