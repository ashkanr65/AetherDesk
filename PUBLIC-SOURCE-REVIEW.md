# Public source consolidation — 20 October 2026

The maintainer authorized publishing complete source, adding a project licence,
and keeping source and releases together in ashkanr65/AetherDesk on main.

Before publication, all 14 then-reachable source commits (494 historical blobs)
and the working tree were scanned for private-key blocks, GitHub/provider/cloud
key formats, credential URLs and assigned secrets. The five reported locations
were inspected: deliberately invalid credentials/private-key text in redaction
and URL rejection tests, and a fixed terminal framing token. No live credential
was detected. Signing material stays in Windows Credential Manager / encrypted
Actions secrets; only the existing verification public key is tracked. This is a
scoped automated/manual review, not a guarantee that no undiscovered issue exists.

Original code already declared MIT OR Apache-2.0 in both Cargo workspaces.
LICENSE, LICENSE-MIT and LICENSE-APACHE supply that choice explicitly. Existing
third-party notices and their licence terms remain applicable. The build copies
project licences and collects dependency declarations/supplied licence files;
GStreamer and bundled X11 notices remain part of the package. This review does
not claim a full legal audit of every transitive dependency.

The existing public repository receives source history so existing release tags,
asset URLs, signatures, digests and the update feed remain intact. Older build
1/2 tags retain their historical downloads-only source archive; future tags use
the exact source main commit. The old source repository is retained as an
archived recovery copy rather than deleting history. Local source/kit/data paths
remain unchanged. Source pushes do not trigger compilation or publication.

Current validation: Rust parser/formatting, migration catalog checks and offline
recipe/publisher/signature fixtures only. No native compilation, package build,
new release publication, real upgrade replacement or clean-machine acceptance
is claimed for this candidate. Run the next build and live checks before release.
