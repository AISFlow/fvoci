use crate::auth::token::SESSION_TTL_SECS;
use crate::error::SESSION_COOKIE;

pub fn set_session_cookie(secure: bool, token: &str) -> String {
    let secure_flag = if secure { "; Secure" } else { "" };
    format!(
        "{name}={value}; HttpOnly; Path=/; SameSite=Lax; Max-Age={max_age}{secure}",
        name = SESSION_COOKIE,
        value = token,
        max_age = SESSION_TTL_SECS,
        secure = secure_flag,
    )
}

pub fn clear_session_cookie(secure: bool) -> String {
    let secure_flag = if secure { "; Secure" } else { "" };
    format!(
        "{name}=; HttpOnly; Path=/; SameSite=Lax; Max-Age=0{secure}",
        name = SESSION_COOKIE,
        secure = secure_flag,
    )
}
