# Code signing policy

Free code signing provided by [SignPath.io](https://about.signpath.io/),
certificate by [SignPath Foundation](https://signpath.org/).

> **Status:** releases are unsigned until the SignPath Foundation accepts SDM
> into its program for open-source projects.

## What gets signed

The Windows release files: the app (`SDM.exe`) and its installer
(`SDM-Setup-<version>.exe`). Both are built by
[GitHub Actions](.github/workflows/build.yml) on GitHub-hosted runners from a
version tag in this repository, with no cached build output, and every signing
request is approved by hand before anything is signed.

## Team roles

- **Committers and reviewers:** [Kevin Liu (@kevl0317)](https://github.com/kevl0317)
- **Approvers:** [Kevin Liu (@kevl0317)](https://github.com/kevl0317)

Changes from anyone outside the team are reviewed by a team member before they
are merged. All team members use multi-factor authentication for GitHub and
SignPath.

## Privacy policy

This program will not transfer any information to other networked systems
unless specifically requested by the user or the person installing or
operating it.

SDM connects only to the servers hosting the downloads you start or accept.
With clipboard catching on, it reads the links you copy, on your computer. The
optional browser extension hands the links you click, with that site's cookies
so signed-in downloads work, to SDM on your computer over a local connection;
nothing is sent anywhere else.
