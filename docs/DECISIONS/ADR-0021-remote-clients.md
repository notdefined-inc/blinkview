# ADR-0021: remote clients, and what "nothing leaves the machine" means under one

Date: 2026-08-31
Status: Accepted

## Context

ADR-0001's premise is that nothing leaves the machine: the map never fetches a tile
because a tile request tells a server where the user has been, and there is no
telemetry. The roadmap kept phones out of the product for the same reason a native
mobile port was rejected (ADR-0009, ADR-0010): the phone does not hold the library.

But the library being on the desktop is also the thing that hurts. Showing someone
photographs means gathering around one screen. The roadmap's "Later: phone apps as
*clients*" always meant connecting a phone to the desktop over the network, the way
Immich and Ente do — which is pixels and commands leaving the machine, over a network,
on purpose. The premise needs to survive that, or the feature should not exist.

## Decision

**Remote access is a first-class mode, and the privacy premise is restated rather
than abandoned: nothing readable leaves the machine except to a device the user
explicitly paired, and no third party ever gains the ability to read anything.**

* The bridge is the third peer over one engine (ARCHITECTURE.md), not a parallel
  implementation: it dispatches the same commands through the same core functions,
  the same Plan previews, the same journal and undo.
* Pairing is explicit, per-enablement, and revocable: a fresh 128-bit token behind a
  QR code, every route gated on it, a lockout on repeated failures, a visible
  Disconnect. Disabled means nothing is listening.
* Sequence: LAN plaintext first (token-gated; the residual risk — a hostile local
  network — is accepted and documented), then end-to-end encryption of the channel
  with key material carried in the QR, then an internet relay. **The relay is only
  permitted as a carrier of ciphertext.** A relay that could read photograph bytes,
  query text, or people's names would void this ADR regardless of what the spec that
  built it said.
* The no-network features stay no-network. The map still never fetches a tile; models
  are still fetched only by the user asking; the release check still sends only a
  versioned user-agent request. Remote mode adds a listener the user turned on; it
  does not add a caller.

## Consequences

"Nothing leaves the machine" stops being literally true the moment the toggle is on,
and the docs must say so plainly rather than keep a claim the binary no longer honours.
The token gate is load-bearing from day one — which is why plaintext-LAN ships only
behind it, and why the E2E spec is written before the relay spec even though the
relay is what users will eventually ask for. The bridge inherits every safety rule at
the core layer (dry-run plans, journalled writes, peek refusals) for free, and the
tests assert that inheritance rather than trusting it.
