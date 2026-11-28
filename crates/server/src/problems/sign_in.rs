//! The descriptions of the reasons of signing in through an organization's provider,
//! of linking an account to it and of the organization enforcing it.

use crate::error::Reason;

/// The description of [`Reason::OidcUnavailable`].
const OIDC_LOGIN_PAGE_UNAVAILABLE: &str = "Status 409 at the start of a sign-in through an \
    organization's provider; on the login page after one, whose reasons come back in the \
    `oidc_error` parameter of its address, by these names. This one: the organization has \
    no provider that is on, or the provider could not be reached, or it was switched off \
    or changed while the person was signing in; or the sign-in cannot end for a reason of \
    the server's: the client secret does not open with its key, the account of the \
    identity was deleted or changed meanwhile, no user name was free, or the server \
    failed. The server's log says which.";

/// The descriptions of the reasons of signing in through a provider.
pub(super) fn describe_sign_in(reason: Reason) -> Option<&'static str> {
    Some(match reason {
        Reason::EmailUnverified => {
            "Status 403 from a login whose password is right, on a server that asks a \
             registration to confirm its email address (`email_verification`): the \
             registration has not been confirmed, and there is no account until the link \
             mailed to the address confirms it, within seven days. A wrong password is \
             answered as for no account."
        }
        Reason::PasswordRequired => {
            "Status 403 from `completeEmailVerification`, for a link that would confirm a \
             registration, opened in another browser than the one it was made from: send \
             the request again with `password`, the password the registration was made \
             with. The link stays as it was. Only who registered finishes a registration, \
             so that nobody is signed in to an account whose password somebody else \
             chose; `errors` names `password`. Status 409 from `leaveOrganization`: the \
             account has no password (`hasPassword` of `getMe` is false) and must set one \
             (`setPassword`) before leaving an organization. Nothing changed."
        }
        Reason::SecretKeyRequired => {
            "Status 409. The server seals the client secret of a provider with its key, \
             `FVOCI_SECRET_KEY`, and has none: a provider cannot be stored until the server is \
             started with one."
        }
        Reason::OidcIssuerUnverified => {
            "Status 400. The discovery document of the issuer could not be read, or does not \
             name the issuer exactly, or its endpoints are not `https`. The server reaches \
             only `https` addresses, follows no redirect, and reaches only public addresses \
             unless its configuration lets it reach others. `errors` names `/issuer`; \
             `detail` says which of these it was, and nothing of the provider's answer."
        }
        Reason::OidcUnavailable => OIDC_LOGIN_PAGE_UNAVAILABLE,
        Reason::OidcStateInvalid => {
            "On the login page, after a sign-in through a provider. What came back is no \
             sign-in this browser started, or one that was used already or took longer than \
             ten minutes, or it came back to the address of another organization than the one \
             it was started for, or from another issuer (`iss`); or it was started to link \
             and the session it was started from has ended, or is not the one the browser \
             came back with. Start again from the login page."
        }
        Reason::OidcDenied => {
            "On the login page, after a sign-in through a provider: the provider sent the \
             person back without letting them sign in."
        }
        Reason::OidcTokenRefused => {
            "On the login page, after a sign-in through a provider: the provider did not \
             give an ID token for the code, or could not be reached for it."
        }
        Reason::OidcTokenInvalid => {
            "On the login page, after a sign-in through a provider: its ID token does not \
             hold (signature, algorithm, key, issuer, audience, time, nonce). The server's \
             log says which."
        }
        Reason::OidcEmailMissing => {
            "On the login page, after a sign-in through a provider: the person has no \
             account yet, and the provider gave no email address to make one with; or the \
             sign-in was to link, and the provider gave none to know the account by."
        }
        Reason::OidcEmailUnverified => {
            "On the login page, after a sign-in through a provider: the person has no \
             account yet, or the sign-in was to link, and the provider says it has not \
             checked their email address."
        }
        Reason::OidcEmailTaken => {
            "On the login page, after a sign-in through a provider: the person has no \
             account through the provider, and an account with a password has the email \
             address the provider gave as its own. The two are not joined by their \
             address: sign in with the password of that account, and link the provider to \
             it there (`startOidcLogin` with `linkPassword`); after that the provider \
             signs in to the same account. An address a provider gave for another account \
             is no account's own, and refuses nobody."
        }
        Reason::OidcJoinClosed => {
            "On the login page, after a sign-in through a provider: the person has no place \
             in the organization (no member of it, nor a guest of one of its workspaces), \
             and the organization does not let newcomers in through its provider."
        }
        other => return describe_linking_and_enforcing(other),
    })
}

/// The descriptions of the reasons of linking a provider to an account, and of an
/// organization that takes no password.
fn describe_linking_and_enforcing(reason: Reason) -> Option<&'static str> {
    Some(match reason {
        Reason::OidcIdentityTaken => {
            "On the login page, after a sign-in through a provider that was started to \
             link (`startOidcLogin` with `linkPassword`): who the provider vouches for is \
             another account's already. Nothing is linked, and the two accounts are not \
             joined."
        }
        Reason::OidcTrialAccount => {
            "On the organization's page, after a trial of its provider's pending settings \
             (`startOidcTrial`): the identity the provider vouched for is not the \
             administrator's who started the trial (it is another account's, or a deleted \
             account's, or tied to none where the settings are of the same issuer and so \
             tie nothing), or that administrator administers the organization no more. Nothing is \
             changed, no account is made and no identity is tied: settings no \
             administrator signs in through are not put in use."
        }
        Reason::OidcTrialStale => {
            "On the organization's page, after a trial of its provider's pending settings \
             (`startOidcTrial`) that are of another issuer: the identity that issuer \
             vouched for is tied to no account yet, and the trial ties it to the \
             administrator's own only when it was started from a session made through \
             the organization's provider, as it is stored, and the browser came back \
             with that session no more than five minutes after it was made. Who only \
             holds an older session, and does not sign in through the provider, ties \
             nothing. Nothing is changed. Sign in through the organization's provider \
             again (`startOidcLogin`), and finish the trial within five minutes \
             (`bindsIdentityUntil` of the pending settings says until when)."
        }
        Reason::OidcCarryOverRefused => {
            "Status 409 at `startOidcCarryOver` and `finishHeldOidcLogin`, and on the login \
             page after a sign-in through the organization's former provider. While the \
             provider an organization moved away from is kept (`former` of its provider), \
             a sign-in of somebody who has no account is held for the browser for ten \
             minutes, to be carried over to the account they had (by signing in through \
             the former provider once) or to end in a new account. This is the one answer \
             to everything that keeps that from happening: no sign-in is held for this \
             browser, or it ended or was taken already; the former provider is kept no \
             more; the provider changed meanwhile; the former provider vouched for nobody \
             who has an account with a place in the organization (a member, or a guest of \
             one of its workspaces in use); or the held identity \
             was tied to another account meanwhile. Nothing is changed: no identity is \
             tied and no session made. Start again from the login page."
        }
        Reason::OidcEmailMismatch => {
            "On the login page, after a sign-in through a provider that was started to \
             link (`startOidcLogin` with `linkPassword`): the email address the provider \
             vouches for is not the account's own. Nothing is linked: the account's \
             password ties to it only the person the provider knows by the account's \
             address. A provider that gave no address, or one it has not checked, is \
             refused as at a first sign-in (`oidc_email_missing`, \
             `oidc_email_unverified`)."
        }
        Reason::PasswordNotSet => {
            "Status 409, linking a provider to the account or removing an identity from \
             it: both ask for the account's password, and the account has none (it was \
             made by a sign-in through a provider)."
        }
        Reason::LastSignIn => {
            "Status 409, removing an identity from the account (`unlinkIdentity`): the \
             account has no password and no other identity, so nothing else would let \
             its owner in."
        }
        Reason::SsoRequired => {
            "Status 401, with a session that is good: the organization takes no password \
             (`ssoRequired` of its provider), and the session was not made through its \
             provider. Everything of the organization answers so (the routes under \
             `/w/{workspaceId}` and `/org/{orgId}`, and `getMyMemberships` and \
             `setUserAdmin`, which name neither and are of the organization the server \
             stands in, and `acceptInvitation`, for a link into it, member or not), from \
             the request after it was switched on; the session \
             stays, and opens what is of other organizations. `orgId` names the \
             organization: start a sign-in through its provider (`startOidcLogin`) and the \
             session it ends in opens it. `getMyWorkspaces` says beforehand which \
             workspaces are closed so. A board's public link is read without a session and \
             is not closed."
        }
        Reason::SsoUnproven => {
            "Status 409, setting an organization's provider. Taking no password \
             (`ssoRequired`) is switched on by a session that was made through that very \
             provider, switched on: who asks has then shown that it lets them in. Sign in \
             through the provider and send the request again."
        }
        Reason::SsoEnforced => {
            "Status 409, setting or removing an organization's provider. While the \
             organization takes no password (`ssoRequired`), its provider is not switched \
             off, removed, or set to another issuer or client: nobody would get in. \
             Switch `ssoRequired` off first. A refused setting names in `errors` what of \
             it was refused; a refused removal names nothing. The client secret can \
             still be set, and a wrong one lets nobody in either. If nobody gets in through the provider any more, who runs the \
             server releases the organization on its administration socket \
             (`POST /api/v1/admin/orgs/{orgId}/sso/release`)."
        }
        _ => return None,
    })
}
