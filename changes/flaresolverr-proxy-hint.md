### Added

- FlareSolverr can be marked as already behind the VPN (Outside services → FlareSolverr →
  Already behind the VPN, `MELETE_FLARESOLVERR_BEHIND_VPN`, off by default), as one sharing
  the VPN container's network is: an indexer set to go through the proxy can then use it, and
  it's handed no proxy.

### Changed

- An indexer that needs FlareSolverr but can't use it through the proxy (a proxy with a
  login, an HTTPS proxy, an older FlareSolverr) now says what to do: turn the proxy off for
  it, or mark FlareSolverr as behind the VPN.
