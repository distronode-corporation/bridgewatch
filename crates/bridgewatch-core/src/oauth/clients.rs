//! The OAuth applications this build of bridgewatch ships with.
//!
//! ⚠️ These are NOT registered yet. While a value is `None`, the wizard and the
//! Settings window do not offer "Sign in" for that host unless the user types a
//! client id of their own, `config validate` asks for one, and `auth login`
//! refuses with the same sentence. Filling one in is the whole of switching
//! sign-in on for that host: nothing else in the tree names a client id.
//!
//! A client id is not a secret. Device-flow applications have no client
//! secret at all (GitHub: "Enable Device Flow" on a GitHub App; GitLab: an
//! application that is NOT confidential, scope `read_api`), which is what
//! makes shipping one in a public binary correct.

/// The client id of bridgewatch's GitHub App on github.com, e.g. `Iv23li...`.
pub const GITHUB_COM: Option<&str> = Some("Iv23linCrkhVaN0BiYBU");

/// The URL slug of that GitHub App, as in
/// `https://github.com/apps/<slug>/installations/new`: where a user installs
/// the app on an organisation whose repositories it cannot see yet.
pub const GITHUB_APP_SLUG: Option<&str> = Some("bridgewatch-ci");

/// The application id (GitLab calls the client id that) of bridgewatch's OAuth
/// application on gitlab.com.
pub const GITLAB_COM: Option<&str> =
    Some("34359b8c0970a5166211c80fe800514c8753dafd81da90f298026ba28e5a4d5a");

/// The built-in applications, as one value.
///
/// Everything that decides whether sign-in is available takes one of these
/// rather than reading the constants, so the tests can prove both the shipped
/// state (all `None`) and the registered one without a second build.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BuiltinClients {
    /// [`GITHUB_COM`].
    pub github_com: Option<&'static str>,
    /// [`GITHUB_APP_SLUG`].
    pub github_app_slug: Option<&'static str>,
    /// [`GITLAB_COM`].
    pub gitlab_com: Option<&'static str>,
}

impl BuiltinClients {
    /// What this build ships.
    pub const fn shipped() -> Self {
        Self {
            github_com: GITHUB_COM,
            github_app_slug: GITHUB_APP_SLUG,
            gitlab_com: GITLAB_COM,
        }
    }
}
