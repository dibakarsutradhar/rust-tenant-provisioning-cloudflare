use anyhow::{Result, anyhow};

#[derive(Clone, Debug)]
pub struct Config {
    // database
    pub database_url: String,

    // app
    pub host: String,
    pub port: u16,

    // domain
    pub base_domain: String,
    pub app_subdomain: String, // e.g. "app" → app.thegarageos.com

    // jwt
    pub jwt_secret: String,
    pub jwt_expiry_days: i64,

    // cloudflare
    pub cloudflare_api_token: String,
    pub cloudflare_zone_id: String,
    pub cloudflare_tunnel_id: String,
    pub mock_cloudflare: bool,

    // rate limiting
    pub rate_limit_register_max: i32,    // requests
    pub rate_limit_register_window: i64, // seconds
    pub rate_limit_login_max: i32,
    pub rate_limit_login_window: i64,

    // provisioning
    pub provision_cname_max_attempts: u32,
    pub provision_cname_poll_secs: u64,
    pub provision_ssl_max_attempts: u32,
    pub provision_ssl_poll_secs: u64,
    pub provision_txt_max_attempts: u32,
    pub provision_txt_poll_secs: u64,
    pub provision_cname_dns_address: String,

    // db pool
    pub db_max_connections: u32,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        dotenvy::dotenv().ok();

        Ok(Self {
            // database
            database_url: require("DATABASE_URL")?,

            // app
            host: optional("APP_HOST", "0.0.0.0"),
            port: optional("APP_PORT", "8080")
                .parse()
                .map_err(|_| anyhow!("APP_PORT must be a number"))?,

            // domain
            base_domain: optional("BASE_DOMAIN", "thegarageos.com"),
            app_subdomain: optional("APP_SUBDOMAIN", "app"),

            // jwt
            jwt_secret: require("JWT_SECRET")?,
            jwt_expiry_days: optional("JWT_EXPIRY_DAYS", "7")
                .parse()
                .map_err(|_| anyhow!("JWT_EXPIRY_DAYS must be a number"))?,

            // cloudflare
            cloudflare_api_token: optional("CLOUDFLARE_API_TOKEN", ""),
            cloudflare_zone_id: optional("CLOUDFLARE_ZONE_ID", ""),
            cloudflare_tunnel_id: optional("CLOUDFLARE_TUNNEL_ID", ""),
            mock_cloudflare: optional("MOCK_CLOUDFLARE", "true") == "true",

            // rate limiting
            rate_limit_register_max: optional("RATE_LIMIT_REGISTER_MAX", "5")
                .parse()
                .map_err(|_| anyhow!("RATE_LIMIT_REGISTER_MAX must be a number"))?,
            rate_limit_register_window: optional("RATE_LIMIT_REGISTER_WINDOW_SECS", "3600")
                .parse()
                .map_err(|_| anyhow!("invalid RATE_LIMIT_REGISTER_WINDOW_SECS"))?,
            rate_limit_login_max: optional("RATE_LIMIT_LOGIN_MAX", "10")
                .parse()
                .map_err(|_| anyhow!("RATE_LIMIT_LOGIN_MAX must be a number"))?,
            rate_limit_login_window: optional("RATE_LIMIT_LOGIN_WINDOW_SECS", "300")
                .parse()
                .map_err(|_| anyhow!("invalid RATE_LIMIT_LOGIN_WINDOW_SECS"))?,

            // provisioning timeouts
            provision_cname_max_attempts: optional("PROVISION_CNAME_MAX_ATTEMPTS", "40")
                .parse()
                .map_err(|_| anyhow!("invalid PROVISION_CNAME_MAX_ATTEMPTS"))?,
            provision_cname_poll_secs: optional("PROVISION_CNAME_POLL_SECS", "30")
                .parse()
                .map_err(|_| anyhow!("invalid PROVISION_CNAME_POLL_SECS"))?,
            provision_ssl_max_attempts: optional("PROVISION_SSL_MAX_ATTEMPTS", "40")
                .parse()
                .map_err(|_| anyhow!("invalid PROVISION_SSL_MAX_ATTEMPTS"))?,
            provision_ssl_poll_secs: optional("PROVISION_SSL_POLL_SECS", "15")
                .parse()
                .map_err(|_| anyhow!("invalid PROVISION_SSL_POLL_SECS"))?,
            provision_txt_max_attempts: optional("PROVISION_TXT_MAX_ATTEMPTS", "40")
                .parse()
                .map_err(|_| anyhow!("invalid PROVISION_TXT_MAX_ATTEMPTS"))?,
            provision_txt_poll_secs: optional("PROVISION_TXT_POLL_SECS", "30")
                .parse()
                .map_err(|_| anyhow!("invalid PROVISION_TXT_POLL_SECS"))?,
            provision_cname_dns_address: optional("PROVISION_CNAME_DNS_ADDRESS", "@1.1.1.1"),

            // db pool
            db_max_connections: optional("DB_MAX_CONNECTIONS", "5")
                .parse()
                .map_err(|_| anyhow!("invalid DB_MAX_CONNECTIONS"))?,
        })
    }

    pub fn app_base_url(&self) -> String {
        format!("https://{}.{}", self.app_subdomain, self.base_domain)
    }

    pub fn tunnel_cname_target(&self) -> String {
        format!("{}.cfargotunnel.com", self.cloudflare_tunnel_id)
    }

    pub fn is_app_host(&self, host: &str) -> bool {
        host == format!("{}.{}", self.app_subdomain, self.base_domain)
    }
}

fn require(key: &str) -> Result<String> {
    std::env::var(key).map_err(|_| anyhow!("{key} must be set in .env"))
}

fn optional(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}
