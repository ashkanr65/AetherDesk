# Security reports

Please report suspected credential exposure, host-key bypass, unsafe file access,
update verification bypass or unintended command execution privately. Do not
include passwords, private keys, API keys, WireGuard private keys, design files
or complete process dumps in a public issue.

Use GitHub's private vulnerability reporting for this repository when the
Security tab offers it. If it is unavailable, contact the repository maintainer
privately through the contact information on their GitHub profile before posting
technical details. Repository administrators must enable private reporting in
GitHub; adding this document does not enable that service.

Include the version **and build**, installed or portable package type, operating
systems, affected connection mode, a minimal reproduction and redacted logs.
Keep the original evidence locally. Request a private transfer method before
sharing sensitive diagnostics.

Only the latest published release is intended to receive security fixes. There
is no separate long-term-support branch or guaranteed response deadline.

Update-manifest signatures and Windows executable certificate signatures serve
different purposes. Never bypass an update signature or hash failure. AetherDesk
uses the permissions of the authenticated host account; it does not make user
startup commands, AI-approved commands or monitoring scripts a security sandbox.
