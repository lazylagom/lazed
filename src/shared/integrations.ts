import { invoke } from "@tauri-apps/api/core";

/** Connected accounts — provider metadata is UI-side; the backend stores
 * sites (non-secret fields) in integrations.json and each site's `token`
 * in the Keychain. Values reach spawned shells as env vars, only where
 * the ambient env doesn't already define them. */
export interface IntegrationFieldSpec {
  key: string;
  label: string;
  placeholder: string;
  secret?: boolean;
  /** not required for a working connection (e.g. server username) */
  optional?: boolean;
  /** helper text under the field — `link` renders as a link to `url` */
  hint?: { text: string; link?: string; url?: string };
}

/** Auth-mode picker in the connect modal — each tab's `id` is stored on
 * the site as `fields.kind` so the backend knows how to authenticate. */
export interface IntegrationTabSpec {
  id: string;
  label: string;
  fields: IntegrationFieldSpec[];
}

export interface IntegrationProviderSpec {
  id: string;
  label: string;
  /** subtitle while nothing is connected */
  sub: string;
  /** where to get the token — shown as "learn more" */
  docs_url?: string;
  /** long paragraph under the account-scope card (not-connected body) */
  blurb?: string;
  /** body of the "Account scope" card */
  scope_note?: string;
  /** footer under the site list once connected */
  sites_note?: string;
  connect_label?: string;
  add_label?: string;
  modal_title?: string;
  modal_sub?: string;
  /** lock line at the bottom of the connect modal */
  lock_note?: string;
  tabs: IntegrationTabSpec[];
}

export const INTEGRATION_PROVIDERS: IntegrationProviderSpec[] = [
  {
    id: "jira",
    label: "Jira",
    sub: "Browse, create, and start work from Jira Cloud issues.",
    docs_url: "https://id.atlassian.com/manage-profile/security/api-tokens",
    blurb:
      "Connect a Jira Cloud site with an API token, or a self-hosted Jira with a personal access token or username and password. Credentials are stored locally — tokens live in the macOS Keychain.",
    scope_note:
      "Credentials and account checks for this provider are owned by this desktop client.",
    sites_note:
      "Each connected Jira site has one token stored in the macOS Keychain.",
    connect_label: "Connect Jira",
    add_label: "Add Jira site",
    modal_title: "Connect Jira site",
    modal_sub:
      "Use a Jira Cloud site URL, Atlassian email, and API token to browse issues.",
    lock_note:
      "Your token is stored in the macOS Keychain and never written to disk.",
    tabs: [
      {
        id: "cloud",
        label: "Atlassian Cloud",
        fields: [
          {
            key: "base",
            label: "Jira Cloud site URL",
            placeholder: "https://example.atlassian.net",
          },
          {
            key: "email",
            label: "Atlassian email",
            placeholder: "you@example.com",
          },
          {
            key: "token",
            label: "API token",
            placeholder: "Atlassian API token",
            secret: true,
            hint: {
              text: "Create a token in ",
              link: "Atlassian account settings.",
              url: "https://id.atlassian.com/manage-profile/security/api-tokens",
            },
          },
        ],
      },
      {
        id: "server",
        label: "Self-hosted",
        fields: [
          {
            key: "base",
            label: "Jira server URL",
            placeholder: "https://jira.example.com",
          },
          {
            key: "username",
            label: "Username",
            placeholder: "optional — leave empty to use a PAT",
            optional: true,
          },
          {
            key: "token",
            label: "Personal access token",
            placeholder: "PAT, or password when a username is set",
            secret: true,
            hint: {
              text: "Create a token in your Jira profile under Personal Access Tokens.",
            },
          },
        ],
      },
    ],
  },
];

/** One connected account under a provider — the token never leaves the
 * app, only `token_set` is reported. */
export interface IntegrationSiteState {
  id: string;
  label?: string | null;
  configured: boolean;
  token_set: boolean;
  fields: Record<string, string>;
}

/** What the backend reports per provider. `env` lists the vars this
 * provider contributes (fed by the first configured site). */
export interface IntegrationState {
  provider: string;
  configured: boolean;
  env: string[];
  sites: IntegrationSiteState[];
}

export interface IntegrationTestResult {
  ok: boolean;
  detail?: string;
  error?: string;
  /** site title reported by the provider — used as the site label */
  title?: string;
}

export const integrations = {
  list: () =>
    invoke<{ integrations: IntegrationState[] }>("integration_list").then(
      (r) => r.integrations,
    ),
  save: (
    provider: string,
    /** absent = create a new site */
    site: string | null,
    fields: Record<string, string>,
    /** absent = keep stored token, "" = clear */
    token?: string,
  ) =>
    invoke<IntegrationState>("integration_save", {
      input: { provider, site, fields, token },
    }),
  remove: (provider: string, site: string) =>
    invoke("integration_delete", { provider, site }),
  test: (provider: string, site: string) =>
    invoke<IntegrationTestResult>("integration_test", { provider, site }),
  /** validate unsaved credentials — the connect modal probes before save */
  probe: (provider: string, fields: Record<string, string>, token: string) =>
    invoke<IntegrationTestResult>("integration_probe", {
      input: { provider, fields, token },
    }),
};

/** Editor's "requires" probe — reports what spawned shells actually see. */
export interface DepsCheckResult {
  bins: Record<string, string | null>;
  env: Record<string, boolean>;
}

export const depsCheck = (req: { bins?: string[]; env?: string[] }) =>
  invoke<DepsCheckResult>("deps_check", { input: req });

/** Install hints for known CLI deps — keyed by binary name. */
export const BIN_INSTALL_HINTS: Record<string, string> = {
  gh: "brew install gh",
  glab: "brew install glab",
  jira: "brew install ankitpokhrel/jira-cli/jira",
};

/** env var prefix → provider that can supply it (drives the editor's
 * "configure in Integrations" link). */
export const ENV_PROVIDER_PREFIX: [prefix: string, provider: string][] = [
  ["JIRA_", "jira"],
];
