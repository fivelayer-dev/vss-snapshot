# Changelog

All notable changes to this crate are listed here. Before 1.0, a minor version may break the
API.

## Unreleased

- First version: snapshots, writer status and metadata, and backup and restore through writers.
- `SnapshotSet::lost` names the snapshots that have been removed since they were taken, and
  closing a set doesn't fail because one has.
