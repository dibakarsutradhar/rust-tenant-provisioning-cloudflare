use jsonwebtoken::{DecodingKey, EncodingKey, Header, Validation, decode, encode};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::config::Config;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Claims {
    pub sub: Uuid, // user_id
    pub tenant_id: Uuid,
    pub role: String,
    pub exp: usize, // expiry timestamp
}

pub fn issue(
    config: &Config,
    user_id: Uuid,
    tenant_id: Uuid,
    role: &str,
) -> Result<String, String> {
    let exp = chrono::Utc::now()
        .checked_add_signed(chrono::Duration::days(config.jwt_expiry_days))
        .unwrap()
        .timestamp() as usize;

    let claims = Claims {
        sub: user_id,
        tenant_id,
        role: role.to_string(),
        exp,
    };

    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(config.jwt_secret.as_bytes()),
    )
    .map_err(|e| e.to_string())
}

pub fn verify(config: &Config, token: &str) -> Result<Claims, String> {
    decode::<Claims>(
        token,
        &DecodingKey::from_secret(config.jwt_secret.as_bytes()),
        &Validation::default(),
    )
    .map(|data| data.claims)
    .map_err(|e| e.to_string())
}
