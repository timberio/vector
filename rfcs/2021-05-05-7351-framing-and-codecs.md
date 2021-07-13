# RFC 7351 - 2021-05-05 - Framing and Codecs

This RFC discusses a standardized way of specifying framing and codecs across
sources and sinks. _Framing_ is concerned with turning sequences of bytes into
byte frames (which indicate the boundaries of a complete message). Codecs
consist of _decoders_, which deserialize a byte frame into structured data, and
_encoders_, which serialize structured data into a byte frame.

Conceptually, we want reusable pieces of logic which would allow us to collapse
a source + (decoder) transform into a source with decoder config, and to
collapse an (encoder) transform + sink into a sink with encoder config, in a way
that is transparent to the user. These transforms merely convert between
encoding formats, e.g. from `bytes` to `json` and vice versa.

## Scope

The scope of this RFC concern at which level framing and decoding operates, how
these framers and codecs can be configured, and how they can be shared in a
uniform way. It does not cover any specific implementation for framing or a
codec. Also see [future work](#future-work) for extended goals that are not
covered by this RFC.

## Motivation

Currently, we have no explicit abstraction that is responsible for handling
framing and codecs, such that each source and sink may implement them in their
own way. This can introduce inconsistencies and additional maintenance burden.
Components have their own defaults, making behavior unpredictable and surprising
users, e.g. as documented in
[#3453](https://github.com/timberio/vector/issues/3453).

## Internal Proposal

Since decoding and encoding should apply universally to all sources and sinks,
it should be configurable by a shared field. This is accomplished by the
`SourceOuter` and `SinkOuter` wrappers:

```rust
// Newly introduced wrapper, analogously to `SinkOuter`.
struct SourceOuter {
    framing: Framer,
    decoding: Decoder,
}
```

```rust
struct SinkOuter {
    // … existing fields
    encoding: Encoder,
}
```

Conceptually, decoding is applied _after_ a source has processed an event and
encoding is applied _before_ a sink has processed an event. These can be
implemented as `FunctionTransform`s in our pipeline, in the same way transforms
are implemented.

Framing takes place before an event is created since the message boundary
required to form an event is not known yet. Therefore the source needs to be
aware of framing and call the framer to determine message boundaries before
creating an event.

To make the source aware of framing, the `Framer` needs to be passed to the
`SourceContext`, so that the source implementation can call it:

```rust
struct SourceContext {
    // … existing fields
    framing: Framer,
}
```

In the context of Vector, decoders and encoders are implemented as `Transform`.
That is, they either implement the `TaskTransform` or `FunctionTransform` trait.

Implementing framers as `Transform` would be possible - however, we want to
restrict them to `FunctionTransform` at the moment. The reason here is that
`TaskTransform`s can take any amount of input bytes before they output frames.
If we would allow that, it wouldn't be clear how we merge metadata from multiple
packages that compose a frame, e.g. the sender in UDP datagrams or partition key
and offset in kafka messages. Using `FunctionTransform`, we know that a 1:n
relationship between byte chunks and frames exist, and we can simply duplicate
the metadata for each frame.

## Doc-level Proposal

Sinks and sources will have additional `framing` and `codec` fields, e.g.:

```yaml
framing = "octet"
codec = ["syslog", { type = "json", target_field = "foo" }]
```

The specific short-hand names and options are defined in the implementation of
framing / codec plugins, which are registered centrally and can be referenced
from the Vector config.

## Rationale

One often request feature is reading JSON-encoded messages from Kafka.
Currently, this can only be accomplished by configuring and wiring together two
separate components, where with codecs it could be a simple one-line change to
the config.

Introducing framing/codecs, a source's implementation may also be reused
internally. One example would be the `syslog` source (see
[#7046](https://github.com/timberio/vector/pull/7046)), or the upcoming `syslog`
sink in [#7106](https://github.com/timberio/vector/issues/7106). Instead of
re-implementing socket-based connection handling, the `syslog` components could
be replaced by the `socket` counterparts combined with `octet-framing`. This
reduces a possible source of bugs and inconsistencies and therefore leads to
less maintenance burden.

Introducing codecs may also shrink unnecessary noise in config files by removing
transform steps / input indirections, when basic transforms were used that are
only concerned with encoding formats.

## Prior Art

[Tokio Codecs](https://docs.rs/tokio-util/0.6.7/tokio_util/codec/index.html)
provide traits to conveniently convert from `AsyncRead`/`AsyncWrite` to
`Stream`/`Sink`. These are currently used in custom implementations of sources
to frame byte streams. However, these codecs can only operate on byte input and
would therefore be unsuitable to implement codecs that can be chained.

[Logstash Codec Plugins](https://www.elastic.co/guide/en/logstash/current/codec-plugins.html)
are interesting since they operate on a higher level than what has been proposed
in this RFC. They don't distinguish between a framing and codec stage, but
rather have codecs that support framing (e.g. `line` codec), compression (e.g.
`gzip_lines` codec) and encoding (e.g. `protobuf` codec). Supporting these kind
of codecs could be an interesting future thought but would require an
architectural change, especially to the internal representation of an `Event`.

## Drawbacks

It is possible that the proposed abstraction is too rigid, in a sense that it is
not possible to cleanly separate these stages into "framing" and "codec"
responsibilites. This limits what codecs can do, e.g. applying framing after
decoding/decompression would not be possible as there are no means to "go back"
to the framing stage. However, the proposed solution is still strictly better
than the status quo, as it provides a consistent interface.

For the components that we expect to benefit most from the reusability of
framing and codecs, will they always only differ by encoding in the future?
Would it be possible that these components must be separated again since the
overhead of maintaining an artificial abstraction is higher than having
purposely-built components?

## Future Work

In this first release, we plan to implement a variety of framing and codec
options that cover the most common use cases. However, users might want to use a
custom codec that only applies to their specific use case or is not supported by
Vector yet. Adding a system that would allow custom-defined codecs could be
considered in the future, for now this is can be accomplished by the `wasm` or
`lua` transform.

## Plan Of Attack

- Implement common structures for `Framer`s and `Codec`s in topology builder and
  source/sink context
- Integrate framing into sources and codecs into topology for sources and sinks
- Implement commonly used framers and codecs that are centrally registered and
  can be resolved by name from Vector configs
