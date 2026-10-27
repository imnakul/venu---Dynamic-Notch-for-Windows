# Code signing

Without a signature, Windows SmartScreen shows "Windows protected your PC"
and lists the publisher as unknown when someone runs `venu-setup-*.exe`.
Two things change that:

- **The Microsoft Store build.** Microsoft signs Store packages, so it
  installs with no warning. Nothing to set up here.
- **A code-signing certificate for the GitHub release.** GitHub can't sign
  anything itself. The release workflow can hand the app and the installer to
  [SignPath](https://signpath.io), whose foundation signs open-source projects
  for free.

## How the workflow signs

`.github/workflows/release.yml` signs only when signing is set up. Until then
it builds and publishes unsigned binaries, exactly as before. When set up:

1. `cargo build` builds `venu.exe`; it is uploaded as a workflow artifact and
   sent to SignPath, and the signed copy replaces it.
2. Inno Setup packs the **signed** `venu.exe` into the installer.
3. The installer is signed the same way, then both are published.

## One-time setup

1. **Apply** for free open-source signing with the SignPath Foundation
   (signpath.org). The project must stay open source (Venu is MIT) and be
   built by GitHub Actions from this repository, which it already is.
2. Once approved, in SignPath:
   - install the SignPath GitHub App on this repository and add GitHub as a
     trusted build system for the project;
   - create the project and a signing policy (e.g. `release-signing`);
   - set the project's default artifact configuration to sign every `.exe`
     in the uploaded artifact:

     ```xml
     <?xml version="1.0" encoding="utf-8"?>
     <artifact-configuration xmlns="http://signpath.io/artifact-configuration/v1">
       <zip-file>
         <pe-file path="*.exe">
           <authenticode-sign />
         </pe-file>
       </zip-file>
     </artifact-configuration>
     ```

   - create an API token for a user with submitter rights on the policy.
3. In GitHub, **Settings › Secrets and variables › Actions**:

   | Kind     | Name                           | Value                              |
   | -------- | ------------------------------ | ---------------------------------- |
   | Secret   | `SIGNPATH_API_TOKEN`           | the API token                      |
   | Variable | `SIGNPATH_ORGANIZATION_ID`     | your SignPath organization ID      |
   | Variable | `SIGNPATH_PROJECT_SLUG`        | the project slug, e.g. `venu`      |
   | Variable | `SIGNPATH_SIGNING_POLICY_SLUG` | the policy slug                    |

4. Re-run the release workflow (Actions › Release › Run workflow) or bump the
   version. The log says "Code signing is not set up" while any of these is
   missing.

SignPath Foundation asks signed projects to state their code signing policy
where users download the app. Once approved, add their wording to the README.

## What to expect

- Signed files show the publisher (the SignPath Foundation certificate names
  the project) instead of "Unknown publisher".
- SmartScreen also weighs reputation. A newly signed app can still see a
  warning for its first downloads; it fades as people install it. No
  certificate type skips this any more.
- The uninstaller Inno Setup writes at install time is not signed by this
  flow. Windows does not warn when it runs from Apps & features.
