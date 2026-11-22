# Security Policy

## Supported versions

Only the latest release gets security fixes.

## Reporting a vulnerability

Report privately via the repository's **Security** tab → **Report a vulnerability**.
Never include session files, tokens or passwords.

## Past issues

- **0.3.0:** fixes dependency advisories in releases up to 0.2.6, notably
  RUSTSEC-2026-0344 (`ruzstd`, via `aurelia-steam-cdn`).
- **0.1.37:** session moved to the OS keyring. Don't use older versions.
- **0.1.37-2:** fixes a bug that could delete `steamapps/common`.
