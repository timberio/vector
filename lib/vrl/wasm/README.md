# vrl-wasm

This directory houses the assets for compiling a [WebAssembly][wasm] binary for [Vector Remap Language][vrl].

## Prerequisites

* Install [wasm-pack]
* Install the `wasm32-unknown-unknown` target for Rust

## Build

To build a Wasm binary runnable in the browser:

```bash
wasm-pack build --target web
```

[Other targets][targets] are available as well.

That outputs a variety of files in `pkg`.

## Using the module from JavaScript

This Wasm module offers just one function, `resolve`, which takes a single JSON object with two fields:

* `program` is the VRL input program (string)
* `event` is the VRL event that you want to transform (JSON object)

Here's an example usage of the binary from JavaScript:

```javascript
import { resolve } from 'vrl-wasm.js';

const program = `.name = "Lee Benson"`;

const event = {
  game: "GraphQL"
};

const input = {
  program: program,
  event: event
};

const res = resolve(input);

const expected = {
  // Value returned by the last expression in the program
  output: "\"Lee Benson\"",
  // The transformed event
  result: {
    name: "Lee Benson",
    game: "GraphQL"
  }
}
```

## Testing

To run the tests:

```bash
wasm-pack test --node
```

[vrl]: https://vrl.dev
[wasm]: https://webassembly.org
[wasm-pack]: https://github.com/rustwasm/wasm-pack
[targets]: https://rustwasm.github.io/wasm-pack/book/commands/build.html#target
