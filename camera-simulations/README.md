# Camera simulations

Simulators of three EO/IR (visible + thermal) cameras, each speaking its vendor's protocol over
UDP. No video is sent; stream addresses are reported as metadata only. The vendors' documents are
in [`../specs`](../specs).

| Simulator | Camera                     | Port  | Vendor document                  |
| --------- | -------------------------- | ----- | -------------------------------- |
| `bx20`    | Brightmere BX-20           | 14580 | `Brightmere_BX-20_ICD_RevC.pdf`  |
| `vlr10`   | Optronique Vallerin VLR-10 | 14581 | `NT-0317_VLR-10_indice_D.pdf`    |
| `lc30t`   | Lucen LC-30T               | 9003  | `LC-30T_Protocol_V1.1.3.pdf`     |

## Installation

The simulators need Rust 1.87 or newer. Install Rust with [rustup](https://rustup.rs), then build:

```sh
cargo build --release
```

## Running

```sh
cargo run --release -- bx20
cargo run --release -- vlr10
cargo run --release -- lc30t
```

Each simulator listens on `0.0.0.0:<port>` and replies to the address a datagram came from. Run
`cargo run -- --help` to list the simulators.
