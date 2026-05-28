// Shared constant for the Google Cloud setup walkthrough (R5.5).
//
// The doc lives at `docs/google-cloud-setup.md` in the repo and is
// not bundled into the app distribution; we link to the canonical
// hosted copy on GitHub. Both `DriveErrorScreen.svelte` and the
// "Comment configurer Google Drive ?" link in `Settings.svelte`
// reference it through this constant so a path/branch change is a
// single-line update.
//
// Kept as a `.ts` file (rather than inlining in each component) so
// future test files can import it without spinning up a Svelte
// component test rig.

export const GOOGLE_CLOUD_SETUP_DOC_URL =
  "https://github.com/qobee/qobee/blob/main/docs/google-cloud-setup.md";
