use crate::error::AppError;

pub(crate) const MIN_PROXY_LISTEN_PORT: u16 = 1024;
pub(crate) const MAX_PROXY_LISTEN_PORT: u16 = u16::MAX;

pub(crate) fn is_valid_proxy_listen_address(value: &str) -> bool {
    if value.is_empty() {
        return false;
    }
    if matches!(value, "localhost" | "0.0.0.0") {
        return true;
    }

    let parts = value.split('.').collect::<Vec<_>>();
    if parts.len() != 4 {
        return false;
    }

    parts
        .iter()
        .all(|part| !part.is_empty() && part.parse::<u8>().is_ok())
}

pub(crate) fn validate_proxy_listen_address(address: &str) -> Result<(), AppError> {
    if is_valid_proxy_listen_address(address) {
        return Ok(());
    }

    Err(AppError::InvalidInput(
        "proxy listen address must be localhost, 0.0.0.0, or an IPv4 address".to_string(),
    ))
}

pub(crate) fn validate_proxy_listen_port(port: u16) -> Result<(), AppError> {
    if (MIN_PROXY_LISTEN_PORT..=MAX_PROXY_LISTEN_PORT).contains(&port) {
        return Ok(());
    }

    Err(AppError::InvalidInput(format!(
        "proxy listen port must be between {MIN_PROXY_LISTEN_PORT} and {MAX_PROXY_LISTEN_PORT}"
    )))
}

#[cfg(test)]
mod tests {
    use super::is_valid_proxy_listen_address;

    #[test]
    fn proxy_listen_address_validation_matches_tui_rules() {
        for value in ["localhost", "0.0.0.0", "127.0.0.1", "192.168.1.20"] {
            assert!(
                is_valid_proxy_listen_address(value),
                "{value} should be valid"
            );
        }

        for value in ["", "127.0.0", "127.0.0.999", "example.com", "::1"] {
            assert!(
                !is_valid_proxy_listen_address(value),
                "{value} should be invalid"
            );
        }
    }
}
