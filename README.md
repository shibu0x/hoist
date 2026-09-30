# transit

Upload and download files to cloud storage from the terminal. Google Drive
today; S3, Dropbox and others planned.

- Resumable uploads and downloads — a dropped connection continues where it left off
- Multiple accounts, switch between them
- Interactive file picker with search, so you never have to type a path
- Refresh tokens in your OS keyring, not a plaintext file

## Install

```sh
git clone <this repo> && cd transit
cargo install --path .
```

## Setup

transit uses **your own** Google OAuth client rather than a shared built-in one:
your own API quota, your own consent screen, no third party in the middle. It's
a one-time setup — see [Getting Google credentials](#getting-google-credentials).

```sh
transit account add     # paste your Client ID + Secret, then approve in the browser
```

## Usage

```sh
transit upload ./report.pdf          # upload a file
transit upload                       # browse and search for one instead
transit list                         # what have I uploaded?
transit download report.pdf          # by name
transit download 'https://drive.google.com/file/d/1AbC.../view'
transit download 1AbC... --out ~/Downloads/report.pdf
```

Quote Drive links in zsh — the `?` in them is a glob character.

### Accounts

```sh
transit account add        # authorise another account
transit account list       # connected accounts, * marks active
transit account switch     # choose which account commands act on
transit account remove     # revoke access and delete stored credentials
```

### OAuth client

```sh
transit client show        # which client is configured, and where it came from
transit client set         # enter or replace the client ID and secret
```

An **account** is a Google user whose Drive you're reading and writing. The
**client** is your app registration in Google Cloud. One client serves every
account, so removing an account keeps the client — you reconnect without
re-entering it.

### Interactive picker

`transit upload` with no path opens a browser for your local filesystem:

```
? /Users/you/Developer ›
  ..
  [ search this folder and below ]
  projects/
  banner.jpg   219.99 KiB
```

Type to filter instantly. Enter descends into a folder or picks a file, `..`
goes up, Esc or `[ cancel ]` backs out. The search entry walks the tree below
the current folder when you don't know where a file is — capped at depth 6 and
200 hits so searching from `~` stays responsive. Hidden files are skipped; pass
an explicit path to upload one.

### Resuming

Uploads go up in 8 MiB chunks against a Drive resumable session, and the
session URI is saved, so an interrupted transfer resumes rather than restarting:

```sh
transit upload big.zip     # Ctrl-C at 50%
transit upload big.zip     # Resuming at 50%
```

The resume offset comes from Drive, not from a local byte count — after a
dropped connection, only the server knows what it actually kept. Downloads
resume the same way via a `Range` request when a partial file is on disk.

Sessions are keyed by path, size and mtime, so editing a file starts a fresh
upload rather than resuming onto stale bytes. Drive expires sessions after about
a week.

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
13. Copy the **Client ID** and **Client Secret**. You can look them up again
    later from the same page.
14. Go back to **OAuth consent screen** and click **Publish app**, then
    **Confirm**.

Then run `transit account add` and paste the two values when prompted.

### Step 14 is not optional

While the app sits in **Testing**, Google expires refresh tokens after **7
days** and caps you at 100 users — you would have to reconnect every week.
Publishing fixes it. `drive.file` is a non-sensitive scope, so publishing does
not put you through Google's verification review.

### Why `drive.file` and not full Drive access

`drive.file` grants access only to files transit creates. It cannot read the
rest of your Drive, and `transit list` only ever shows files this tool
uploaded. If you revoke access, everything else is untouched.

The tradeoff: you cannot browse or download files you created elsewhere. A Doc
you made in the browser is invisible to transit and `download` will return a
404. Changing that needs the full `drive` scope and fresh consent.

## Where things are stored

Everything lives in `~/.config/transit/` (`0700`), each file a map keyed by
provider or account — so adding a provider adds entries, not files.

| What | Where | Keyed by |
|---|---|---|
| Client ID + Secret | `credentials.json` (`0600`) | provider |
| **Refresh token** | **OS keyring** — Keychain / Credential Manager / Secret Service | account |
| Access token (~1 hour) | `cache.json` (`0600`) | account |
| Upload sessions | `uploads.json` (`0600`) | account + file |

The refresh token is the only credential that is both permanent and powerful,
so it is the only one in the keyring, where reads are gated by a per-app ACL.
The access token expires on its own within the hour, so it is cached on disk —
that is what stops every command from triggering a keyring prompt. If no keyring
is available (headless Linux with no Secret Service), refresh tokens fall back
to `tokens.json` at `0600`.

### Overrides

Every override follows `TRANSIT_<PROVIDER>_<THING>`, checked before anything on
disk.

| Variable | Effect |
|---|---|
| `TRANSIT_GOOGLE_CLIENT_ID` + `TRANSIT_GOOGLE_CLIENT_SECRET` | use this OAuth client (both required) |
| `TRANSIT_GOOGLE_REFRESH_TOKEN` | use this refresh token, skip the keyring entirely |

The refresh-token override is the development escape hatch: every `cargo build`
relinks the binary with a new code signature, and macOS ties a Keychain item's
ACL to the calling binary — so "Always Allow" stops applying after a rebuild.

```sh
export TRANSIT_GOOGLE_REFRESH_TOKEN=$(security find-generic-password -s transit -a google:you@gmail.com -w)
```

`$(...)` keeps the token out of your shell history, and `/usr/bin/security` is
a stable binary, so granting *it* Keychain access sticks.

## Security

- **PKCE (S256)** and a random `state` parameter on the OAuth flow: an
  intercepted authorisation code is useless, and a forged callback is rejected.
- The callback server binds `127.0.0.1` on a **random free port**, so no other
  local process can squat a known port to race for the code.
- `account remove` **revokes the grant with Google** before deleting anything
  locally — the token is the only thing that can authorise its own revocation.
- Tokens are never printed, logged, or included in error output.

Anyone who can read `~/.config/transit/` as your user can act as you on files
this tool created. Revoke any time at
[myaccount.google.com/permissions](https://myaccount.google.com/permissions).

## Resetting

```sh
transit account remove            # one account, revoked and cleaned up
rm -rf ~/.config/transit          # everything, including the OAuth client
```

## Not yet supported

Folders (everything uploads to Drive root), deleting remote files, Google Docs
export, headless auth (the browser flow needs a display), and providers other
than Google Drive.
