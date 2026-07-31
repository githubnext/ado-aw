---
name: "ado-aw candidate smoke: multi-repo checkout"
description: "Proves Azure DevOps places multi-checkout repositories where the compiler says, and that the baked self identity matches the running repository"
target: standalone
pool:
  name: AZS-1ES-L-Playground-ubuntu-22.04
engine:
  id: copilot
  model: gpt-5-mini
  timeout-minutes: 5
permissions:
  read: agent-playground-read
repos:
  # A genuinely different repository from `self`, so the two checkouts cannot
  # contend for the agent's per-repository cache directory. (Checking out the
  # same repo twice makes ADO try to move one cached copy to both paths, which
  # warns and forces a re-clone every run.) Pinned to its stable default
  # branch — the `e2e/*` branches in this repo are scratch refs owned by the
  # executor-e2e suite and may be rewritten.
  - name: ado-aw-e2e-fixture
    alias: e2e-fixture
    ref: refs/heads/main
    fetch-depth: 1
safe-outputs:
  noop: {}
  # Configured but never invoked. `create-pull-request` is what makes the
  # SafeOutputs job replicate the Agent job's multi-checkout layout (issue
  # #1731), so declaring it is how this fixture reaches Stage 3 coverage. The
  # agent is told to emit only `noop` and is granted no `edit` tool, so there
  # is nothing to propose and no write ever reaches ADO.
  create-pull-request:
    # `target-branch` is the fallback for EVERY checked-out repo, so it must
    # name a branch that exists in each of them. `ado-aw-smoke-candidate-base`
    # exists only in the mirror (`self`); the additional checkout needs its own
    # override or `prepare-pr-base` warns that it cannot resolve a merge-base.
    target-branch: ado-aw-smoke-candidate-base
    target-branches:
      e2e-fixture: main
steps:
  - bash: |
      set -euo pipefail

      echo "checkout root:"
      ls -la "$CHECKOUT_ROOT"

      # 1. Both repositories must exist at the exact paths the compiler emitted
      #    (`path: s/self` and `path: s/e2e-fixture`).
      if [ ! -d "$SELF_DIR/.git" ]; then
        echo "expected the self checkout at $SELF_DIR" >&2
        exit 1
      fi
      if [ ! -d "$FIXTURE_DIR/.git" ]; then
        echo "expected the additional checkout at $FIXTURE_DIR" >&2
        exit 1
      fi

      self_head="$(git -C "$SELF_DIR" rev-parse HEAD)"
      fixture_head="$(git -C "$FIXTURE_DIR" rev-parse HEAD)"
      echo "self HEAD=$self_head"
      echo "fixture HEAD=$fixture_head"

      # 2. `self` must be the exact candidate commit the orchestrator queued.
      if [ "$self_head" != "$SOURCE_VERSION" ]; then
        echo "self is at $self_head, expected candidate $SOURCE_VERSION" >&2
        exit 1
      fi

      # 3. The two checkouts are different repositories, so equal commits would
      #    mean they collapsed into one directory.
      if [ "$self_head" = "$fixture_head" ]; then
        echo "additional checkout matches self; the checkouts collided" >&2
        exit 1
      fi

      # 4. The workflow source must be reachable beneath the self checkout:
      #    this is the path Stage 3 passes to `ado-aw execute --source`.
      if [ ! -f "$LOCK_FILE" ]; then
        echo "compiled pipeline missing beneath $SELF_DIR" >&2
        exit 1
      fi

      # 5. The `self` repository identity is resolved at COMPILE time and baked
      #    into the lock. Prove the baked value matches the repository this
      #    build is actually running against — a silent mismatch here is what
      #    would send a `repository: self` safe output to the wrong repo.
      baked="$(sed -n 's/^ *ADO_AW_SELF_REPOSITORY_NAME: *//p' "$LOCK_FILE" | head -n 1)"
      if [ -z "$baked" ]; then
        echo "no baked self repository identity found in $LOCK_FILE" >&2
        exit 1
      fi
      # Written without the macro's leading '$(' so ADO cannot expand it here.
      case "$baked" in
        *Build.Repository.Name*)
          echo "self identity fell back to the trigger-scoped macro: $baked" >&2
          exit 1
          ;;
      esac

      remote_url="$(git -C "$SELF_DIR" remote get-url origin)"
      actual="${remote_url##*/}"
      echo "baked self repository=$baked actual=$actual"
      if [ "$baked" != "$actual" ]; then
        echo "baked self repository '$baked' does not match '$actual'" >&2
        exit 1
      fi

      echo "multi-repo checkout layout and self identity verified"
    displayName: Assert multi-repo checkout layout and self identity
    env:
      CHECKOUT_ROOT: $(Build.SourcesDirectory)
      SELF_DIR: $(Build.SourcesDirectory)/self
      FIXTURE_DIR: $(Build.SourcesDirectory)/e2e-fixture
      SOURCE_VERSION: $(Build.SourceVersion)
      LOCK_FILE: $(Build.SourcesDirectory)/self/tests/compiler-smoke-e2e/multi-repo.lock.yml
---

## Multi-repo checkout smoke

The pipeline verified its own checkout layout and `self` identity before you
were started. Your only job is to emit a single proof-of-life safe output.

Call **exactly one** safe-output tool:

1. `noop`

   - context: "ado-aw-smoke-$(Build.BuildId)-multi-repo checkout layout verified"

Do not call any other tool. In particular do **not** call
`create-pull-request`: it is configured only so the compiler emits the
multi-checkout SafeOutputs job shape, and there are no changes to propose.
After the safe output is emitted, stop.
