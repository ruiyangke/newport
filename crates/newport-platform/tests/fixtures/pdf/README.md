# PDF renderer fixtures

`plain.pdf` is a generated one-page document containing a red rectangle and the
text "Newport PDF preview". It is also embedded in the app's `--check-pdf` probe.
No document actions, attachments, scripts, or external resources are present.

The encrypted variants were generated with qpdf from `plain.pdf`:

```sh
qpdf --encrypt '' fixture-owner 256 --extract=n --modify=none --print=none -- plain.pdf copy-restricted.pdf
qpdf --encrypt fixture-reader fixture-owner 256 -- plain.pdf password.pdf
```

These public fixture passwords are not credentials. The first document must
render without exposing its restricted text; the second must report that it
requires a password. Ordinary tests need neither qpdf nor a password prompt.
