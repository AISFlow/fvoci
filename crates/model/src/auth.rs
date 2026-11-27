//! Login, registration and password-change requests.

use std::fmt;

use crate::allowlist::utf16_len;
use crate::name::{self, Named};
use crate::problem::json_pointer;
use crate::refusal::{Refusal, RefusalReason, Refusals};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// The minimum length of a password, in characters (Unicode scalar values).
pub const MINIMUM_PASSWORD_LENGTH: usize = 8;

/// The maximum length of a password, in bytes. Far more than anyone types; it keeps a
/// request from making the server hash megabytes.
pub const MAXIMUM_PASSWORD_BYTES: usize = 1024;

/// The longest email address, in bytes (RFC 5321).
pub const MAXIMUM_EMAIL_BYTES: usize = 254;

/// The longest display name, in UTF-16 code units as the web app counts text.
pub const DISPLAY_NAME_MAX_LENGTH: usize = 64;

/// Characters that a display name refuses beside the control characters: those that
/// show nothing (the Hangul fillers and the blank Braille pattern among them, which are
/// letters or symbols), break a line or turn the direction of the text around, with which
/// a name could look empty, or like another.
const INVISIBLE: [std::ops::RangeInclusive<char>; 13] = [
    '\u{00AD}'..='\u{00AD}',
    '\u{034F}'..='\u{034F}',
    '\u{061C}'..='\u{061C}',
    '\u{115F}'..='\u{1160}',
    '\u{180E}'..='\u{180E}',
    '\u{2800}'..='\u{2800}',
    '\u{3164}'..='\u{3164}',
    '\u{FFA0}'..='\u{FFA0}',
    '\u{200B}'..='\u{200F}',
    '\u{2028}'..='\u{202E}',
    '\u{2060}'..='\u{206F}',
    '\u{FEFF}'..='\u{FEFF}',
    '\u{FFF9}'..='\u{FFFB}',
];

fn is_invisible_or_control(c: char) -> bool {
    c.is_control() || INVISIBLE.iter().any(|range| range.contains(&c))
}

/// Reads a display name a user chooses: without the white space around it, empty for
/// none; else at most [`DISPLAY_NAME_MAX_LENGTH`] UTF-16 code units without control
/// characters (PostgreSQL stores no U+0000) and without characters that show nothing,
/// break a line or turn the text around. Returns the name to store.
///
/// # Errors
///
/// Returns an [`AuthParamError`] for `displayName` if the name is too long or holds a
/// character it may not.
pub fn display_name(raw: &str) -> Result<String, AuthParamError> {
    let name = raw.trim();
    if utf16_len(name) > DISPLAY_NAME_MAX_LENGTH {
        return Err(AuthParamError::new(
            "displayName",
            RefusalReason::TooLong,
            format!("a display name has at most {DISPLAY_NAME_MAX_LENGTH} characters"),
        ));
    }
    if name.chars().any(is_invisible_or_control) {
        return Err(AuthParamError::new(
            "displayName",
            RefusalReason::InvalidCharacters,
            "a display name has no control characters, and none that show nothing or turn \
             the text around",
        ));
    }
    Ok(name.to_owned())
}

/// The longest name of an organization, in UTF-16 code units as the web app counts text.
pub const ORGANIZATION_NAME_MAX_LENGTH: usize = 64;

/// Reads the name of the organization somebody starts by signing up, which is the
/// name of its first workspace too, by the rule of [`crate::required_name`].
///
/// # Errors
///
/// Returns an [`AuthParamError`] for `organizationName` if the name is blank, too long or
/// holds a character it may not.
pub fn organization_name(raw: &str) -> Result<String, AuthParamError> {
    name::required_name(raw, Named::Organization)
}

// An organization is named as a person is: one rule for both.
const _: () = assert!(
    ORGANIZATION_NAME_MAX_LENGTH == DISPLAY_NAME_MAX_LENGTH,
    "an organization's name is read by the rule of a display name"
);

/// A request parameter of an authentication request is missing or malformed.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{message}")]
pub struct AuthParamError {
    field: &'static str,
    reason: RefusalReason,
    message: String,
}

impl AuthParamError {
    /// Creates an error about the request member `field`, with the cause a client
    /// chooses its words by and a message that is sent to the client as is.
    pub fn new(field: &'static str, reason: RefusalReason, message: impl Into<String>) -> Self {
        Self {
            field,
            reason,
            message: message.into(),
        }
    }

    /// Why the member was refused.
    pub fn reason(&self) -> RefusalReason {
        self.reason
    }

    /// The name of the request member the error is about, as it is spelled on the wire.
    pub fn field(&self) -> &'static str {
        self.field
    }

    /// The same error about another request member: for a rule that several requests
    /// apply to members of different names.
    #[must_use]
    pub fn about(mut self, field: &'static str) -> Self {
        self.field = field;
        self
    }
}

impl From<AuthParamError> for Refusal {
    fn from(error: AuthParamError) -> Self {
        Self {
            pointer: json_pointer(error.field),
            reason: error.reason,
            detail: error.message,
        }
    }
}

/// What a sign-in sends: a user name or an email address, and a password.
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(default)]
pub struct LoginRequest {
    /// The user name to log in with.
    pub username: String,
    /// The email address to log in with, used when no user has the given name.
    pub email: String,
    /// The password of the account that `username` or `email` names.
    pub password: String,
}

impl fmt::Debug for LoginRequest {
    /// Leaves out the password.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LoginRequest")
            .field("username", &self.username)
            .field("email", &self.email)
            .finish_non_exhaustive()
    }
}

/// A request to register a new user.
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(default)]
pub struct RegisterRequest {
    /// The login name.
    pub username: String,
    /// The email address.
    pub email: String,
    /// The password the account is to have: at least 8 characters, and at most 1024
    /// bytes in UTF-8.
    pub password: String,
    /// The name to be shown by; may be left out or empty for none, not `null`. Checked
    /// as `patchMe` checks it: at most 64 UTF-16 code units without control characters.
    #[serde(rename = "displayName")]
    pub display_name: String,
    /// The name of the organization the account starts, and of that organization's
    /// first workspace, read as [`organization_name`] reads it. Left out or blank for
    /// none: the account starts with its own space alone, and may start an
    /// organization later (`createOrganization`).
    #[serde(rename = "organizationName")]
    pub organization_name: String,
    /// The language tag of the screen the request comes from (`ko`, `en-GB`): the
    /// language of the mail that confirms the address, where the server asks for that
    /// (English, Korean); English for any other.
    pub language: String,
}

impl RegisterRequest {
    /// Checks the request's parameters, all of them.
    ///
    /// # Errors
    ///
    /// Returns every member that is refused, in the order of the members: the user name
    /// if it is blank, longer than [`USERNAME_MAX_LENGTH`], or holds `@` or control
    /// characters, the email address if it is blank or not well formed, the display
    /// name if it is not one ([`display_name`]), the password if it is empty,
    /// shorter than [`MINIMUM_PASSWORD_LENGTH`] or longer than
    /// [`MAXIMUM_PASSWORD_BYTES`], and the organization name if there is one and it is
    /// not one ([`organization_name`]).
    pub fn validate(&self) -> Result<(), Refusals> {
        let mut refused = self.account().refusals();
        if let Some(Err(error)) = self.organization().map(organization_name) {
            refused.push(error.into());
        }
        refused.into_result()
    }

    /// The name of the organization the account starts, as the request has it; `None`
    /// for a request that names none (left out, or blank), whose account starts with
    /// its own space alone.
    #[must_use]
    pub fn organization(&self) -> Option<&str> {
        Some(self.organization_name.as_str()).filter(|name| !name.trim().is_empty())
    }

    /// As [`RegisterRequest::validate`], where the request must name an organization:
    /// until a server has its first user, who starts the server's own organization. A
    /// request that names none is refused for that with whatever else is wrong in it.
    ///
    /// # Errors
    ///
    /// As [`RegisterRequest::validate`], and for a name that is left out or blank.
    pub fn validate_as_first(&self) -> Result<(), Refusals> {
        let mut refused = self.account().refusals();
        if let Err(error) = organization_name(&self.organization_name) {
            refused.push(error.into());
        }
        refused.into_result()
    }

    /// The refusal of a request that names no organization where one must be named
    /// ([`RegisterRequest::validate_as_first`]), by itself.
    #[must_use]
    pub fn organization_required() -> Refusals {
        let mut refused = Refusals::new();
        if let Err(error) = organization_name("") {
            refused.push(error.into());
        }
        refused
    }

    /// The request with its account tidied ([`NewAccount::tidied`]) and its organization name
    /// as it is stored if it is one, and empty if it is blank: what is refused stays
    /// as it was sent, to be refused by [`RegisterRequest::validate`].
    #[must_use]
    pub fn tidied(self) -> Self {
        let account = self.account().tidied();
        let organization_name = match self.organization() {
            Some(name) => organization_name(name).unwrap_or(self.organization_name),
            None => String::new(),
        };
        Self {
            username: account.username,
            email: account.email,
            password: account.password,
            display_name: account.display_name,
            organization_name,
            language: self.language,
        }
    }

    /// The account the request asks for, without the organization it starts.
    #[must_use]
    pub fn account(&self) -> NewAccount {
        NewAccount {
            username: self.username.clone(),
            email: self.email.clone(),
            password: self.password.clone(),
            display_name: self.display_name.clone(),
        }
    }

    /// The registration the request asks for, tidied ([`RegisterRequest::tidied`]), if
    /// it is one then ([`RegisterRequest::validate`]).
    ///
    /// # Errors
    ///
    /// As [`RegisterRequest::validate`], of the tidied request.
    pub fn check(self) -> Result<Registration, Refusals> {
        let tidied = self.tidied();
        tidied.validate()?;
        Ok(tidied.into_registration())
    }

    /// As [`RegisterRequest::check`], where the request must name an organization
    /// ([`RegisterRequest::validate_as_first`]).
    ///
    /// # Errors
    ///
    /// As [`RegisterRequest::validate_as_first`], of the tidied request.
    pub fn check_as_first(self) -> Result<Registration, Refusals> {
        let tidied = self.tidied();
        tidied.validate_as_first()?;
        Ok(tidied.into_registration())
    }

    /// The registration of a request that was tidied and found good.
    fn into_registration(self) -> Registration {
        let organization_name = Some(self.organization_name).filter(|name| !name.trim().is_empty());
        Registration {
            account: CheckedAccount(NewAccount {
                username: self.username,
                email: self.email,
                password: self.password,
                display_name: self.display_name,
            }),
            organization_name,
            language: self.language,
        }
    }
}

/// A registration that was tidied and found good ([`RegisterRequest::check`]): the
/// account to make, the organization it starts if it names one, and the language of
/// the screen it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Registration {
    account: CheckedAccount,
    organization_name: Option<String>,
    language: String,
}

impl Registration {
    /// The account to make.
    #[must_use]
    pub fn account(&self) -> &CheckedAccount {
        &self.account
    }

    /// The name of the organization the account starts, as it is stored; `None` for
    /// a registration that names none, whose account starts with its own space alone.
    #[must_use]
    pub fn organization(&self) -> Option<&str> {
        self.organization_name.as_deref()
    }

    /// The language tag of the screen the registration came from, as it was sent.
    #[must_use]
    pub fn language(&self) -> &str {
        &self.language
    }
}

/// The account somebody asks for: who registers, and who accepts an invitation
/// without having one.
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(default)]
pub struct NewAccount {
    /// The login name.
    pub username: String,
    /// The email address.
    pub email: String,
    /// The password the account is to have: at least 8 characters, and at most 1024
    /// bytes in UTF-8.
    pub password: String,
    /// The name to be shown by; may be left out or empty for none, not `null`. Checked
    /// as `patchMe` checks it: at most 64 UTF-16 code units without control characters.
    #[serde(rename = "displayName")]
    pub display_name: String,
}

impl fmt::Debug for NewAccount {
    /// Leaves out the password.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NewAccount")
            .field("username", &self.username)
            .field("email", &self.email)
            .field("display_name", &self.display_name)
            .finish_non_exhaustive()
    }
}

/// Maximum length in Unicode scalar values of [`NewAccount::username`].
pub const USERNAME_MAX_LENGTH: usize = 64;

/// An account asked for that was tidied and found good ([`NewAccount::check`]): what an
/// account is made of.
#[derive(Clone, PartialEq, Eq)]
pub struct CheckedAccount(NewAccount);

impl CheckedAccount {
    /// The login name, without the white space around it.
    #[must_use]
    pub fn username(&self) -> &str {
        &self.0.username
    }

    /// The email address, without the white space around it.
    #[must_use]
    pub fn email(&self) -> &str {
        &self.0.email
    }

    /// The password, as it was sent.
    #[must_use]
    pub fn password(&self) -> &str {
        &self.0.password
    }

    /// The name to be shown by, as it is stored; empty for none.
    #[must_use]
    pub fn display_name(&self) -> &str {
        &self.0.display_name
    }
}

impl fmt::Debug for CheckedAccount {
    /// Leaves out the password, as [`NewAccount`]'s does.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("CheckedAccount").field(&self.0).finish()
    }
}

impl NewAccount {
    /// The account tidied ([`NewAccount::tidied`]), if it is one to make then
    /// ([`NewAccount::validate`]).
    ///
    /// # Errors
    ///
    /// As [`NewAccount::validate`], of the tidied account.
    pub fn check(self) -> Result<CheckedAccount, Refusals> {
        let tidied = self.tidied();
        tidied.validate()?;
        Ok(CheckedAccount(tidied))
    }

    /// The account with its name and address without the white space around them, and
    /// its display name as it is stored if it is one: what is refused stays as it was
    /// sent, to be refused by [`NewAccount::validate`].
    #[must_use]
    pub fn tidied(mut self) -> Self {
        self.email = self.email.trim().to_owned();
        if !self.username.chars().any(is_invisible_or_control) {
            self.username = self.username.trim().to_owned();
        }
        if let Ok(name) = display_name(&self.display_name) {
            self.display_name = name;
        }
        self
    }

    /// Checks the account's members, all of them.
    ///
    /// # Errors
    ///
    /// Returns every member that is refused, in the order of the members: the user name
    /// if it is blank, longer than [`USERNAME_MAX_LENGTH`], or holds `@`, control, or
    /// invisible/direction-override characters, the email address if it is blank or not
    /// well formed, the display name if it is not one ([`display_name`]), and the
    /// password if it is empty, shorter than [`MINIMUM_PASSWORD_LENGTH`] or longer than
    /// [`MAXIMUM_PASSWORD_BYTES`].
    pub fn validate(&self) -> Result<(), Refusals> {
        self.refusals().into_result()
    }

    /// What of the account is refused.
    fn refusals(&self) -> Refusals {
        let mut refused = Refusals::new();
        let trimmed_username = self.username.trim();
        if trimmed_username.is_empty() {
            refused.refuse("username", RefusalReason::Required, "username is required");
        } else if self.username.contains('@') || self.username.chars().any(is_invisible_or_control)
        {
            refused.refuse(
                "username",
                RefusalReason::InvalidCharacters,
                "username must not contain '@' or control characters",
            );
        } else if trimmed_username.chars().count() > USERNAME_MAX_LENGTH {
            refused.refuse(
                "username",
                RefusalReason::TooLong,
                format!("username must be at most {USERNAME_MAX_LENGTH} characters"),
            );
        }
        if self.email.trim().is_empty() {
            refused.refuse("email", RefusalReason::Required, "email is required");
        } else if self.email.len() > MAXIMUM_EMAIL_BYTES {
            refused.refuse(
                "email",
                RefusalReason::TooLong,
                format!("email must be at most {MAXIMUM_EMAIL_BYTES} bytes"),
            );
        } else if !is_email_valid(&self.email) {
            refused.refuse(
                "email",
                RefusalReason::InvalidFormat,
                "invalid email format",
            );
        }
        if let Err(error) = display_name(&self.display_name) {
            refused.push(error.into());
        }
        if let Err(error) = check_new_password(&self.password) {
            refused.push(error.into());
        }
        refused
    }
}

impl fmt::Debug for RegisterRequest {
    /// Leaves out the password.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RegisterRequest")
            .field("username", &self.username)
            .field("email", &self.email)
            .field("display_name", &self.display_name)
            .field("organization_name", &self.organization_name)
            .finish_non_exhaustive()
    }
}

/// A request to change a user's password.
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct ChangePasswordRequest {
    /// The current password.
    pub old_password: String,
    /// The password to change to.
    pub new_password: String,
}

impl ChangePasswordRequest {
    /// Checks the request's parameters, both of them.
    ///
    /// # Errors
    ///
    /// Returns every member that is refused: the old password if it is empty, and the
    /// new one if it is empty, shorter than [`MINIMUM_PASSWORD_LENGTH`] or longer than
    /// [`MAXIMUM_PASSWORD_BYTES`].
    pub fn validate(&self) -> Result<(), Refusals> {
        let mut refused = Refusals::new();
        if self.old_password.is_empty() {
            refused.refuse(
                "oldPassword",
                RefusalReason::Required,
                "old password is required",
            );
        }
        if self.new_password.is_empty() {
            refused.refuse(
                "newPassword",
                RefusalReason::Required,
                "new password is required",
            );
        } else if let Err(error) = check_new_password(&self.new_password) {
            refused.push(error.about("newPassword").into());
        }
        refused.into_result()
    }
}

impl fmt::Debug for ChangePasswordRequest {
    /// Leaves out both passwords.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ChangePasswordRequest")
            .finish_non_exhaustive()
    }
}

/// A request to give an account that has no password its first one.
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct SetPasswordRequest {
    /// The password to sign in with from now on.
    pub password: String,
}

impl SetPasswordRequest {
    /// Checks the password.
    ///
    /// # Errors
    ///
    /// Returns `password` refused if it is empty, shorter than
    /// [`MINIMUM_PASSWORD_LENGTH`] or longer than [`MAXIMUM_PASSWORD_BYTES`].
    pub fn validate(&self) -> Result<(), Refusals> {
        let mut refused = Refusals::new();
        if let Err(error) = check_new_password(&self.password) {
            refused.push(error.into());
        }
        refused.into_result()
    }
}

impl fmt::Debug for SetPasswordRequest {
    /// Leaves out the password.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SetPasswordRequest").finish_non_exhaustive()
    }
}

/// A request of a session to be taken as one that has shown its account's password.
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct ConfirmPasswordRequest {
    /// The account's password.
    pub password: String,
}

impl fmt::Debug for ConfirmPasswordRequest {
    /// Leaves out the password.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConfirmPasswordRequest")
            .finish_non_exhaustive()
    }
}

/// A request of an account to start an organization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NewOrganization {
    /// The name of the organization, and of its first workspace: read as
    /// `organizationName` of `register` is.
    pub name: String,
}

impl NewOrganization {
    /// The name as it is stored: read by the rule of [`organization_name`].
    ///
    /// # Errors
    ///
    /// Returns an [`AuthParamError`] that names `name` if the name is blank, too long
    /// or has a character a name may not have.
    pub fn stored_name(&self) -> Result<String, AuthParamError> {
        organization_name(&self.name).map_err(|error| error.about("name"))
    }
}

/// Checks a password that is about to be set: on registration, on a password change,
/// and when an administrator sets one.
///
/// # Errors
///
/// Returns an [`AuthParamError`] if the password is empty, has fewer than
/// [`MINIMUM_PASSWORD_LENGTH`] characters, or more than [`MAXIMUM_PASSWORD_BYTES`]
/// bytes.
pub fn check_new_password(password: &str) -> Result<(), AuthParamError> {
    if password.is_empty() {
        return Err(AuthParamError::new(
            "password",
            RefusalReason::Required,
            "password is required",
        ));
    }
    if password.chars().count() < MINIMUM_PASSWORD_LENGTH {
        return Err(AuthParamError::new(
            "password",
            RefusalReason::TooShort,
            format!("password must be at least {MINIMUM_PASSWORD_LENGTH} characters"),
        ));
    }
    if password.len() > MAXIMUM_PASSWORD_BYTES {
        return Err(AuthParamError::new(
            "password",
            RefusalReason::TooLong,
            format!("password must be at most {MAXIMUM_PASSWORD_BYTES} bytes"),
        ));
    }
    Ok(())
}

/// Returns `true` if `email` has the shape of an email address.
///
/// The address must be 3 to 254 bytes long: a local part of ASCII letters, digits and
/// ``.!#$%&'*+/=?^_`{|}~-``, an `@`, and a domain of dot-separated labels. Each label is
/// 1 to 63 ASCII letters, digits and hyphens, and neither starts nor ends with a hyphen.
pub fn is_email_valid(email: &str) -> bool {
    if !(3..=MAXIMUM_EMAIL_BYTES).contains(&email.len()) {
        return false;
    }
    let Some((local, domain)) = email.split_once('@') else {
        return false;
    };
    let local_ok = !local.is_empty()
        && local
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b".!#$%&'*+/=?^_`{|}~-".contains(&b));
    local_ok && domain.split('.').all(is_domain_label)
}

fn is_domain_label(label: &str) -> bool {
    let bytes = label.as_bytes();
    let (Some(first), Some(last)) = (bytes.first(), bytes.last()) else {
        return false;
    };
    bytes.len() <= 63
        && first.is_ascii_alphanumeric()
        && last.is_ascii_alphanumeric()
        && bytes
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || *b == b'-')
}
