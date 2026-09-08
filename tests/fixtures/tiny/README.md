# tiny

The smallest useful page: one heading, one paragraph, no scripts, no
subresources, 200 bytes. Used by `S-cold-fetch` (process start to the first
request on the wire) so that the page itself contributes nothing measurable,
and by the `tenta-testutil` server tests as the fixture they fetch.

Expected settled text output: `expected.txt`.
