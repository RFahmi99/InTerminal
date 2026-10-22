# Releasing InTerminal

Releases are built by GitHub Actions (`.github/workflows/release.yml`) when a version tag is pushed. Installed copies of InTerminal find new releases through the updater, which reads `latest.json` from the newest **published** GitHub release.

## Make a release

1. Set the same version in `src-tauri/tauri.conf.json`, `src-tauri/Cargo.toml` and `package.json`, then commit.
2. Create an annotated tag. Its message becomes the "What's new" text shown in InTerminal (it is copied into `latest.json`, so editing the release's notes on GitHub later doesn't change what InTerminal shows):

   ```powershell
   git tag -a v0.2.0 -m "Faster command search and fixes for ..."
   git push origin v0.2.0
   ```

3. Wait for the **Release** workflow. If code signing is set up, approve the two signing requests in SignPath: first the app, then the installer.
4. Check the draft release on GitHub. It holds the installer, its updater signature (`.sig`) and `latest.json`.
5. Publish the draft. Installed copies then offer the update the next time they check.

The workflow refuses to build if the tag doesn't match all three version numbers.

## Updater signing key

Every update is signed with a private key, and InTerminal only installs updates whose signature matches the public key in `tauri.conf.json`.

- The private key and its password are the repository secrets `TAURI_SIGNING_PRIVATE_KEY` and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`.
- The maintainer keeps a backup outside the repository. **If the key is lost, installed copies can't be updated any more**; users would have to download a new installer by hand.
- Never commit the private key or its password.

## Code signing with SignPath Foundation

Until SignPath is set up, releases are built and published unsigned (the workflow shows a warning), so Windows SmartScreen warns people who download the installer.

To turn signing on:

1. The repository must be public, with an OSI-approved licence (MIT) and the "Code signing policy" section in the README.
2. Apply at <https://signpath.org/apply> for the GitHub project. SignPath Foundation reviews open-source projects before accepting them.
3. Once accepted, in the SignPath project:
   - link the GitHub repository as a trusted build system (the GitHub Actions integration), so only artifacts built by this repository's workflows can be signed;
   - set the default artifact configuration to sign every executable in the uploaded zip:

     ```xml
     <artifact-configuration xmlns="http://signpath.io/artifact-configuration/v1">
       <zip-file>
         <pe-file path="*.exe">
           <authenticode-sign />
         </pe-file>
       </zip-file>
     </artifact-configuration>
     ```

   - note the organization ID, project slug and signing policy slug (usually `release-signing`), and create an API token for a CI user that can submit to that policy.
4. In the GitHub repository settings, add:
   - the secret `SIGNPATH_API_TOKEN`;
   - the variables `SIGNPATH_ORGANIZATION_ID`, `SIGNPATH_PROJECT_SLUG` and `SIGNPATH_POLICY_SLUG`.

The next release is then signed. Release signing requests wait for manual approval in SignPath.
