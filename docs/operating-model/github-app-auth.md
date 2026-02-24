# GitHub App Auth for Agent Commits

Date: 2026-02-22
Owner: Repository Owner + Agent
Status: Active draft

## Purpose

Use GitHub App authentication for repository writes so agent pushes are separated from human credentials.

## Required values

Set these environment variables before setup:

- `GH_APP_ID`
- `GH_APP_INSTALLATION_ID`
- `GH_APP_PRIVATE_KEY_PATH`

Recommended (persistent per repository): store these in local git config so you do not need to export them every session.

```bash
git config --local opencode.githubAppId "2922027"
git config --local opencode.githubAppInstallationId "111703951"
git config --local opencode.githubAppPrivateKeyPath "$HOME/.config/opencode/github-app.pem"
```

These values are stored in `.git/config` (local only, not committed).

Optional:

- `GH_APP_TOKEN` (if omitted, token is minted automatically by script)
- `GH_APP_ID`, `GH_APP_INSTALLATION_ID`, `GH_APP_PRIVATE_KEY_PATH` as environment variables (override local config for current shell)

## One-time setup per repository/worktree

Run:

```bash
./scripts/use-github-app-auth.sh
```

This command will:

- Set local commit identity to:
  - `user.name = donut`
  - `user.email = donut-bot@users.noreply.github.com`
- Set origin to token-compatible HTTPS form (`https://x-access-token@github.com/<owner>/<repo>.git`).
- Configure git credentials for `https://github.com` with a GitHub App installation token.

Note:

- GitHub App installation tokens expire (typically about 1 hour).
- Rerun `./scripts/use-github-app-auth.sh` when token expires.

## Verification

Check identity and auth state:

```bash
git config --local user.name
git config --local user.email
git config --local --get remote.origin.url
```

Push smoke test:

```bash
git push --dry-run origin HEAD
```

Verify latest commit author/committer:

```bash
git log -1 --format="Author: %an <%ae>%nCommitter: %cn <%ce>"
```

## Safety rules

- Keep `main` push restricted by branch protection and repository policy.
- Do not store app private keys or tokens in the repository.
- Rotate app key/token if compromise is suspected.

## Troubleshooting

- If token mint fails, verify app permissions include repository `Contents: write`.
- If push fails on protected branch, use feature branch + PR flow.
- If identity is wrong, rerun setup script in the active worktree.
