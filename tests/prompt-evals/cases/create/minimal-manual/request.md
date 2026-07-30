Create an ado-aw workflow named `summarize-repository-todos`.

It should run manually, inspect the checked-out repository for TODO and FIXME
comments, and return a concise grouped summary in the agent response. It must
not create or update anything outside the run. If there are no matches, report
that clearly.

