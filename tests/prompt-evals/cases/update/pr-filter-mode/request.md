Our Azure DevOps repository now has a Build Validation branch policy for this
pipeline. Update the supplied workflow to use PR `mode: policy`, include target
branches `main` and `release/*`, and include paths `src/*` and `docs/*`.
Preserve the body and all unrelated configuration.

