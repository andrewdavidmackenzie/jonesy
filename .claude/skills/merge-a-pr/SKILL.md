---
name: merge-a-pr
description: When work on an issue in a branch with a PR is completed and the human asks to merge the PR
---

When closing work done in a PR for the project:

## Checks

Double-check the points in the original issue were addressed.

Check that documentation was updated to reflect the changes.

Check that integration tests were expanded to cover the changes.

Check that tests and docs were updated to reflect any changes in the panic counts found or 
the reason for the panics.

## Anything left loose

Check there are no uncommitted or unpushed changes or files present locally that are not in revision
control or ignored. That could represent things forgotten, or the user wants to carry over to other
work. Warn the user before causing any change that could lose it.

## Merge the PR

Merge the PR using gh, if it returns an error check to see if the user has already merged it
via the GH user interface or some similar method.

If the user has already merged it, report that, but not as an error and consider the PR 
correctly merged and move on.

## Cleaning up

If everything is OK, then check out the master/main branch and git pull. Check that the commits
in the PR are present and everything is OK.
