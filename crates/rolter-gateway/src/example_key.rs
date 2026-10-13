//! Startup and reload warning for the public `sk-rolter-dev` key (#1965).
//!
//! The key ships in `rolter.example.toml`, the image's baked config and
//! `easy-up`, and it is printed in the README, so anyone can present it. On a
//! loopback bind that is a convenience; on any other bind it is an open door to
//! every model on the operator's provider keys.

use std::net::IpAddr;

use chrono::Utc;
use rolter_core::GatewayConfig;

/// Whether a listener bound to `ip` can be reached from another host.
/// The unspecified address (`0.0.0.0`, `::`) counts as reachable.
pub fn bind_is_exposed(ip: IpAddr) -> bool {
    !ip.is_loopback()
}

/// Whether the public example key is an active virtual key in `config`.
fn carries_public_example_key(config: &GatewayConfig) -> bool {
    let now = Utc::now();
    config
        .virtual_keys
        .iter()
        .any(|k| k.is_public_example_key() && k.is_active(now))
}

/// Log a warning when the public example key is live on an exposed bind.
/// Returns whether it warned. Called at startup and on every reload, so the
/// line repeats for as long as the condition holds.
pub fn warn_if_exposed(config: &GatewayConfig, exposed: bool) -> bool {
    if !exposed || !carries_public_example_key(config) {
        return false;
    }
    tracing::warn!(
        key = rolter_core::PUBLIC_EXAMPLE_KEY,
        "the public virtual key {} is active and this gateway is reachable from other hosts: \
         anyone who has read the README can use every model on your provider keys. Delete its \
         [[virtual_keys]] entry and mint a key of your own, or bind to 127.0.0.1",
        rolter_core::PUBLIC_EXAMPLE_KEY
    );
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use rolter_core::VirtualKeyConfig;

    fn config_with(key: &str) -> GatewayConfig {
        GatewayConfig {
            virtual_keys: vec![VirtualKeyConfig {
                key: key.to_string(),
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    #[test]
    fn loopback_binds_are_not_exposed_and_the_rest_are() {
        assert!(!bind_is_exposed("127.0.0.1".parse().unwrap()));
        assert!(!bind_is_exposed("::1".parse().unwrap()));
        assert!(bind_is_exposed("0.0.0.0".parse().unwrap()));
        assert!(bind_is_exposed("::".parse().unwrap()));
        assert!(bind_is_exposed("10.1.2.3".parse().unwrap()));
    }

    #[test]
    fn the_dev_key_warns_off_loopback_only() {
        let config = config_with(rolter_core::PUBLIC_EXAMPLE_KEY);
        assert!(warn_if_exposed(&config, true));
        assert!(!warn_if_exposed(&config, false));
    }

    #[test]
    fn a_replaced_key_is_silent_everywhere() {
        let config = config_with("sk-rolter-replaced-for-this-test");
        assert!(!warn_if_exposed(&config, true));
        assert!(!warn_if_exposed(&config, false));
    }

    #[test]
    fn a_disabled_dev_key_is_silent() {
        let mut config = config_with(rolter_core::PUBLIC_EXAMPLE_KEY);
        config.virtual_keys[0].disabled = true;
        assert!(!warn_if_exposed(&config, true));
    }
}
