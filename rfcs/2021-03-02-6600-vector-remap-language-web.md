# RFC 6600 - 2021-03-03 - Vector Remap Language on the web

From the beginning of our work on [Vector Remap Language][VRL] (VRL), we've had discussions about
novel ways to make VRL approachable and even downright fun for users. Thus far, we've done some
pretty unique things, like providing a REPL for the language. But we've also had plans, though
fairly unspecific ones, about making VRL available on the web in some form. This RFC seeks to lend
some concreteness to those discussions.

## Options

In this RFC, I discuss three possibities. The first two emerged from our internal discussions, while
the third was gathered from a survey of the Wasm ecosystem.

1. An in-browser [web app][web_app]/playground for experimenting with and learning VRL
2. A [web server][web_server] that can process VRL programs and events
3. A [published Wasm module], largely for internal use

I propose pursuing #1 and #3 in the near term, while leaving #2 as an option for the future.

### Interactive web app

An interactive VRL web app would be a browser-based app that provides an aesthetically pleasing and
UX-friendly experience for learning and experimenting with VRL. In its most basic form, it would
enable you to enter a VRL event and a VRL program into a text area and see what happens. But there
are other capabilities that we should consider:

* A step-by-step tutorial, like the one under way in PR [6184] that provides real exercises with
  right and wrong answers
* A built-in "library" of example events (JSON, Syslog, AWS formats, etc.)
* The ability to share URLs for specific VRL events and/or programs. This would likely require using
  a backend [web server] although it *may* be feasible to use URL hashes to achieve this.

I propose that we begin with a proof of concept that displays the event in its current state and the
output of each program and allows for program input via a textarea.

#### Prior art

In PR [6590] I created a proof of concept for a VRL web app that uses the [Trunk] build tool and the
[Yew] front-end framework for Rust, which is inspired by [React.js][react] and enables you to write
web apps in pure Rust and without touching JavaScript. This POC did "work" but it created an
unacceptably large WebAssembly (Wasm) binary (~15 MB), largely due to it containing both VRL (which
is bulky in itself) and *all* of the app's front-end logic.

A much better solution, I believe, would be to create a web app that's mostly standard HTML, CSS,
and JavaScript and delegating the VRL processing logic to one of the following:

* A Wasm binary that only handles VRL logic
* The [web server] proposed below

### Web server

A VRL web server would be a simple HTTP server with one endpoint to which you can `POST` the
following:

* A VRL program (as a string)
* A VRL event (as JSON)
* (optional) a compiler state. This would require one change to VRL, namely that
  `vrl::compiler::state::Compiler` would need to be serializable/deserializable to/from JSON. With
  that change in place, responsibility for compiler state could be handed over to the client.

Here's an example `POST` payload:

```json
{
  "program": "del(.foo)\n.bar = baz",
  "event": {
    "foo": "bar"
  },
  "state": {
    "variables": {
      "baz": 45.2
    }
  }
}
```

And the response:

```json
{
  "event": {
    "bar": 45.2
  },
  "output": {
    "45.2"
  }
}
```

A server of this kind would be quite germane to "serverless" environments like AWS Lambda and
CloudFlare Workers, as the logic can be made fully stateless (with the change mentioned above).

I propose exploring this option at a later date, as the [web app][web_app] option would provide far
more value in terms of Vector outreach efforts.

#### Implementation

The [Warp] framework would be a natural candidate for a web server. Warp is the only widely used
Rust web framework that both supports async and uses [Tokio] as its async runtime. The server would
only need to have a single endpoint, something like `/compile` or `/resolve`, for an MVP.

#### Related possibilities

* Run a public-facing instance of the server on a platform like Heroku
* A `/functions` endpoint that describes the functions available in the current version (name,
  examples, etc.)
* The ability to share URLs for specific events and/or programs. This would require either making
  the server stateful and using some kind of persistent data store or devising a system for
  inferring events/programs/states via hashes passed in via URL. It's likely that those URLs would
  grow quite unwieldy, however, so we should experiment before taking this route.

### Published module

The [wasm-pack] tool makes it trivial to publish Wasm artifacts to [npm]. We should provide an npm
module called `vrl-wasm` or something similar with versions that correspond to versions of VRL
(e.g. `0.12.*` instead of `0.1.*`). This would largely be for internal use in creating the
[web app][web_app],
though it's not inconceivable that others outside the project might use it as well at some point in
the future by simply running `npm install vrl-wasm`.

## Demonstration of value

When assessing the overall benefit of expanding VRL into the web domain, I believe that the [Open
Policy Agent][opa] (OPA) project is a good place to look. OPA has a DSL called [Rego] that you use
to create policy logic. The maintainers of OPA created an [OPA Playground][opa_playground] that
enables you to experiment with the language using your own custom JSON inputs and policies, a
dramatically less cumbersome experience than using OPA on the command line (although OPA, like VRL,
does have an interactive REPL). In that playground, you can "publish" your policies and share the
resulting URL. One of the core maintainers of OPA confirmed to me that those policies are stored in
Amazon S3.

Rego is a lean but counterintuitive language based on [Datalog]. Having an interactive web
environment seems to be crucial to the success of the language and of its parent project. VRL is
a much less counterintuitive language, yet a VRL playground could nonetheless serve an important
role in boosting public knowledge of Vector at a pretty low engineering cost.

## Plan of attack

The first step here would need to be developing a Wasm module that we're happy with. I've made an
initial foray in this direction in PR [6604]. The module works as expected but could use some
polishing in terms of the interface it exposes as well as the size of the binary (currently ~4 MB
using [`wee_alloc`][wee], which could likely be improved).

Once we have a satisfactory Wasm module, we can build a simple but delightful web app POC around
it. Later, we can explore some of the more advanced options for the web app as well as a stateless
web server.

[6184]: https://github.com/timberio/vector/pull/6184
[6590]: https://github.com/timberio/vector/pull/6590
[6604]: https://github.com/timberio/vector/pull/6604
[datalog]: https://en.wikipedia.org/wiki/Datalog
[npm]: https://npmjs.org
[opa]: https://open-policy-agent.org
[opa_playground]: https://play.openpolicyagent.org
[published wasm module]: #published-module
[react]: https://reactjs.org
[rego]: https://www.openpolicyagent.org/docs/latest/#rego
[tokio]: https://tokio.rs
[trunk]: https://github.com/thedodd/trunk
[vrl]: https://vrl.dev
[vrl_web_app]: https://vrl-web.netlify.app
[warp]: https://github.com/seanmonstar/warp
[wasm]: https://webassembly.org
[wasm-pack]: https://github.com/rustwasm/wasm-pack
[wasmer]: https://wasmer.io
[web_app]: #interactive-web-app
[web_server]: #web-server
[wee]: https://github.com/rustwasm/wee_alloc
[yew]: https://yew.rs
