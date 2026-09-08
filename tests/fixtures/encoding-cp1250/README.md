# encoding-cp1250

A Polish page whose bytes are `windows-1250` and whose only declaration is
`<meta charset="windows-1250">` in the first 1 KiB. It exercises the WHATWG
sniffing path end to end: the prescan picks the encoding, the tokenizer meets
the same declaration and confirms it, and no re-parse happens.

Serve it with `?content_type=text/html` so the response carries no `charset`
parameter: a transport charset is `Certain` and would override the markup,
which is the precedence the transport-charset test asserts separately.

Expected settled text output: `expected.txt` (UTF-8).
