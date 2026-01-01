# Contributing

Thanks for contributing!

## CI Testing Policy

**macOS is a first-class platform.** When modifying CI workflows:

- **DO NOT** disable features or skip tests on macOS just for implementation convenience (e.g., avoiding dependency setup like pnpm).
- **DO** set up the full build environment on each platform so we test what we ship.

Platform-specific exclusions are only acceptable when the feature is **genuinely impossible** on that platform:

| Exclusion     | Rationale                                                         |
| ------------- | ----------------------------------------------------------------- |
| `locald-shim` | Uses Linux namespaces, cgroups, and capabilities (OCI containers) |
| `locald-vmm`  | Uses Linux KVM for microVM support                                |
| `locald-e2e`  | Integration tests requiring the full Linux container stack        |

If a feature should work on macOS, it must be tested on macOS. See `.github/workflows/ci.yml` for the full policy comment.

## Commit signature policy

This repository requires **verified signatures** on commits that land in `main`.

The easiest way to satisfy this is to use **SSH commit signing**.

## Set up SSH commit signing

1. Generate a dedicated signing key:

```sh
ssh-keygen -t ed25519 -C "git-signing (<your-email>)" -f ~/.ssh/id_ed25519_git_signing
```

2. Upload the public key to GitHub as a _signing_ key:

```sh
gh auth refresh -h github.com -s admin:ssh_signing_key
gh ssh-key add ~/.ssh/id_ed25519_git_signing.pub --title "git signing" --type signing
```

3. Configure Git to sign commits and tags by default:

```sh
mkdir -p ~/.config/git
awk '{print "<your-email> " $0}' ~/.ssh/id_ed25519_git_signing.pub > ~/.config/git/allowed_signers

git config --global gpg.format ssh
git config --global user.signingkey ~/.ssh/id_ed25519_git_signing.pub
git config --global gpg.ssh.allowedSignersFile ~/.config/git/allowed_signers

git config --global commit.gpgsign true
git config --global tag.gpgsign true
```

## Troubleshooting / verification

- Verify the most recent commit is signed:

```sh
git log --show-signature -1
```

- Make a one-off unsigned commit (not recommended):

```sh
git commit --no-gpg-sign
```
