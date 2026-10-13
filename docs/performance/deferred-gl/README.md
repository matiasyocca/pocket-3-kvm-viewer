# Deferred GL implementation

Not part of the released app. Do not promote this patch from its screening CPU result alone.

The user chose to stop further optimization work after one bounded GL finishing pass. The isolated implementation reached hardware-free tests and Clippy, but did not deliver a verified release binary. Independent production review, release builds, real-device fallback, compositor suspend/return and packaged verification are still required. No GL feature or dependency was added to the main application.

`shared-gl.patch.gz` preserves the patch exactly against the staged native-YUV/background baseline described in `manifest.json`, not simply against its detached HEAD. Decompress it in a scratch directory; the SHA-256 below applies to the decompressed patch. Use a new isolated worktree and inspect applicability before any future experiment. It contains optional GL selection and bounded recovery, but remains unapproved partial work.

Patch SHA-256: e98e8ffc283e8a10b0bbe5d3e5601fcb137ab04e5d0768e5eb7a484da069838d

`gates.json` preserves reported commands/statuses. Original RED/GREEN and detailed confirmation logs remain in the isolated worktree referenced by the manifest; those scratch paths are not a durable runtime dependency. The manifest's claim of synthetic GL-to-native delivery is not a parent hardware confirmation.

The separately preserved `../pipeline-probe-source.tar.gz` contains the comparison probe's source, manifest/lock, Nix environment, baseline source and CLI documentation. It reproduces the experiment tool, not this unfinished production implementation. Extract into a scratch directory, then follow `docs/PIPELINE_PROBE.md`; hardware/GUI commands are explicit and must not be run alongside an existing viewer.

Probe archive SHA-256: c5757707d85cdf1ce98bc87127c168bd364cf354bb714a16ec4fcc303609c710
