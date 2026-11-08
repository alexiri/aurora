# Instance fixture

An administrator registers the host of a self-hosted deployment first, then members connect to it.
Credentials are only injected when the request host equals the host of the instance the connection
is bound to (or the SaaS default host for a connection without an instance). Test fixture only.
