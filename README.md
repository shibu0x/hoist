# transit

Upload files to cloud storage from the terminal. Google Drive today; Dropbox, S3
and others planned.

## Setup

transit uses **your own** Google OAuth client rather than a shared built-in one.
That means your own API quota, your own consent screen, and no third party in
the middle. It costs you a one-time setup (below) and nothing after that.

```sh
transit config          # pick "Connect GDrive", paste your Client ID + Secret,
                        # approve in the browser that opens
```

You are asked for the Client ID and Secret once. They are stored in
`~/.config/transit/credentials.json` with `0600` permissions and reused on
every later run.

## Usage

```sh
transit config              # connect an account
transit upload ./report.pdf # upload a file
```

## Getting Google credentials

You need a Google Cloud project with the Drive API enabled and a "Desktop app"
OAuth client. Roughly ten minutes, once.

1. Open the [Google Cloud Console](https://console.cloud.google.com/).
2. Create a new project, or select an existing one.
3. Search for `Google Drive API` in the top search bar and open it under
   *Marketplace*.
4. Click **Enable**.
5. Open the **OAuth consent screen** page.
6. Choose **External** user type (Internal is only offered on Workspace
   accounts) and click **Create**.
7. Fill in *App name*, *User support email* and *Developer contact
   information*. Leave the rest empty. If you get `An error saving your app has
   occurred`, pick a more unique app name.
8. Click **Save and continue**.
9. Click **Add or remove scopes**, search for `drive.file`, and select
   `.../auth/drive.file`. Click **Update**, then **Save and continue**.
10. Add your own Google account under **Test users**, then **Save and
    continue**.
11. Open the **Credentials** page. Click **Create credentials** →
    **OAuth client ID**.
12. Application type **Desktop app**. Give it any name and click **Create**.
13. Copy the **Client ID** and **Client Secret** shown. You can look them up
    again later from the same page.
14. Go back to **OAuth consent screen** and click **Publish app**, then
    **Confirm**.

Then run `transit config` and paste the two values when prompted.

### Step 14 is not optional

While the app sits in **Testing**, Google expires refresh tokens after **7
days** and caps you at 100 users. You would have to reconnect every week.
Publishing fixes it. `drive.file` is a non-sensitive scope, so publishing does
not put you through Google's verification review.

### Why `drive.file` and not full Drive access

`drive.file` grants access only to files this tool creates. It cannot read the
rest of your Drive. If you ever revoke access, everything else is untouched.

## Where things are stored

Everything lives in `~/.config/transit/`, one file per concern, each a map
keyed by provider name — so adding Dropbox or S3 reuses the same folder and the
same files rather than introducing new ones.

| What | Where | Shape |
|---|---|---|
| Client ID + Secret | `~/.config/transit/credentials.json` | `{"google": {...}, "dropbox": {...}}` |
| Refresh token | OS keyring (Keychain / Credential Manager / Secret Service) | per provider; falls back to `~/.config/transit/tokens.json` at `0600` |
| Access token | `~/.config/transit/cache.json` | `{"google": {"access_token": ..., "expires_at": ...}}` |

All files are written `0600` inside a `0700` directory.

The refresh token is the long-lived credential, so it goes to the OS keyring
where reads are gated by a per-application ACL. The access token expires on its
own within the hour, so it is cached on disk instead — that is what stops every
upload from triggering a keyring prompt.

### Overrides

Every override follows the same `TRANSIT_<PROVIDER>_<THING>` shape, so the
pattern is identical for each provider you add.

| Variable | Effect |
|---|---|
| `TRANSIT_GOOGLE_CLIENT_ID` + `TRANSIT_GOOGLE_CLIENT_SECRET` | use this OAuth client instead of the stored one (both must be set) |
| `TRANSIT_GOOGLE_REFRESH_TOKEN` | use this refresh token, skip the keyring entirely |

`TRANSIT_GOOGLE_REFRESH_TOKEN` is the escape hatch for CI and for local
development, where every `cargo build` changes the binary's signature and macOS
re-prompts for Keychain access.

## Security

- **PKCE (S256)** and a random `state` parameter on the OAuth flow, so an
  intercepted authorization code is useless and a forged callback is rejected.
- The callback server binds to `127.0.0.1` on a **random free port**, so no
  other local process can squat a known port to race for the code.
- Tokens are never printed, logged, or included in error output.

Anyone who can read your `~/.config/transit/` files as your user can act as you
on files this tool created. Revoke access any time at
[myaccount.google.com/permissions](https://myaccount.google.com/permissions).

## Resetting

```sh
rm -rf ~/.config/transit          # forget credentials and cached tokens
transit config                    # start over
```
