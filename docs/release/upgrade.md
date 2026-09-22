# Release upgrade contract

Oqueue has no durable local state to migrate. A node may be replaced with an
empty local directory; the object store and metadata log are the durable
sources of truth. A rolling upgrade therefore does not copy or transform a
local database.

## Object-format compatibility

Objects are self-describing at their boundary. The bundle, composite manifest,
and partition manifest each carry a format version. A reader that does not
know an **unknown format** must refuse it before decoding fields from it. It
must not guess a previous layout, silently skip the object, or rewrite it in
place. M13.13's `release_upgrade` test exercises all three refusal paths.

The same rule applies to an incompatible release artifact: the release
verification contract refuses an architecture, checksum, signature, or image
binary mismatch before deployment. Compatibility is established by the
artifact and format checks, not by a version string alone.

## Rolling order

1. Validate and stage the new artifact, including its checksum, signature,
   image architecture, startup path, and object-format refusal tests.
2. Upgrade **readers before writers**. During this phase, writers continue to
   emit the format already understood by every live reader.
3. Enable a writer that emits a new object format only after every reader in
   the serving set understands it and the rollback window has been closed.
4. Remove the old binary only after no live object or writer requires its
   format. An old binary encountering a newer format must refuse that object,
   not serve guessed bytes.

Because there is no durable local state, rollback replaces the binary and
restarts the node; it does not require a local-state migration. If a future
format change cannot satisfy readers-before-writers, it requires an explicit
format migration plan and a new compatibility decision before release.
