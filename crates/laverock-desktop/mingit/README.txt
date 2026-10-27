Placeholder for the bundled PortableGit tree.

The real contents are downloaded and extracted by scripts/fetch-mingit.sh
during the Windows release build, then bundled into the installer via
tauri.windows.conf.json. This file only exists so the tauri-build resources
glob matches on a clean checkout.
