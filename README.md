# swap-rs

**swap-rs** is a pure-Rust library for creating Linux swap areas in the
**SWAPSPACE2** format. It can be used both as a library (the `swap-rs` crate)
and as a standalone CLI binary (`mkswap-rs`) that mirrors the behaviour of the
standard `mkswap(8)` utility from **util-linux**.

---

## Features

- **No C dependencies** – only depends on [`uuid`] and [`libc`] crates.
- **Full CLI compatibility** – all `mkswap(8)` flags are supported.
- **Endianness control** – produces swap headers in native, little-endian, or
  big-endian byte order.
- **UUID & label support** – assign any UUID or up-to-16-byte label.
- **Bad-block checking** – `--check` reads every page and records I/O errors.
- **Swap file creation** – `--file` creates and allocates a regular file for
  swapping.
- **Offset & size** – place the swap header at an arbitrary offset or restrict
  the usable area with a size suffix.
- **Library API** – embed swap creation directly in your Rust application.

[`uuid`]: https://crates.io/crates/uuid
[`libc`]: https://crates.io/crates/libc

---

## Library usage

Add `swap-rs` to your `Cargo.toml`:

```toml
[dependencies]
swap-rs = "0.1"
```

### High-level API (`mkswap`)

The simplest way to create a swap area is the [`MkswapConfig`] struct and the
[`mkswap`] function:

```rust
use swap_rs::{MkswapConfig, Endianness, mkswap};

let config = MkswapConfig {
    device: "/dev/sdb1".into(),
    label: Some("linux_swap".into()),
    ..Default::default()
};

mkswap(&config).unwrap();
```

Create a 2 GiB swap file with a specific UUID:

```rust
use swap_rs::{MkswapConfig, mkswap};
use uuid::Uuid;

let uuid_bytes = *Uuid::parse_str("a1b2c3d4-e5f6-7890-abcd-ef1234567890").unwrap().as_bytes();

let config = MkswapConfig {
    device: "/swapfile".into(),
    file: true,
    filesz: 2 * 1024 * 1024 * 1024,  // 2 GiB
    uuid: Some(uuid_bytes),
    label: Some("data_swap".into()),
    ..Default::default()
};

mkswap(&config).unwrap();
```

Little-endian swap with bad-block check and custom page size:

```rust
use swap_rs::{MkswapConfig, Endianness, mkswap};

let config = MkswapConfig {
    device: "/dev/mmcblk0p3".into(),
    endianness: Endianness::Little,
    check: true,
    user_pagesize: 4096,
    force: true,
    ..Default::default()
};

mkswap(&config).unwrap();
```

### Low-level API

For full control, use the underlying primitives directly:

```rust
use swap_rs::*;
use std::fs::{File, OpenOptions};

// Resolve page size and allocate a zeroed signature page
let mut pagesize = 0;
let mut sig_page = init_signature_page(&mut pagesize, 0, false).unwrap();
assert!(pagesize > 0);

// Set the SWAPSPACE2 magic
set_signature(&mut sig_page, pagesize);

// Write header fields
let h = hdr_mut(&mut sig_page);
h.version = cpu32_to_endianness(SWAP_VERSION, Endianness::Native);
h.last_page = cpu32_to_endianness(1048575, Endianness::Native);
h.uuid.copy_from_slice(&[0u8; 16]);

// Open the device and write
let fd = OpenOptions::new().read(true).write(true).open("/dev/sdb1")?;
write_header_to_device(&fd, &sig_page, 0, pagesize)?;
```

Parse human-readable sizes:

```rust
use swap_rs::parse_size;

assert_eq!(parse_size("4K").unwrap(), 4096);
assert_eq!(parse_size("2M").unwrap(), 2 * 1024 * 1024);
assert_eq!(parse_size("1G").unwrap(), 1_073_741_824);
assert_eq!(parse_size("512").unwrap(), 512);
```

Check for bad blocks:

```rust
use swap_rs::{check_blocks, Endianness};
use std::fs::OpenOptions;

let fd = OpenOptions::new().read(true).open("/dev/sdb1").unwrap();
let mut sig_page = vec![0u8; 4096];
let mut nbad = 0;

check_blocks(&fd, 1000, 4096, &mut nbad, &mut sig_page, Endianness::Native, false).unwrap();
println!("Found {} bad pages", nbad);
```

---

## CLI usage

The sibling binary [`mkswap-rs`] wraps this library. Install it:

```bash
cargo install mkswap-rs
```

```
mkswap-rs [options] device [size-in-blocks]
```

### Options

| Option                        | Description                                      |
|-------------------------------|--------------------------------------------------|
| `-c`, `--check`               | Check bad blocks before creating the swap area   |
| `-f`, `--force`               | Allow swap size larger than device; skip boot wipe|
| `-q`, `--quiet`               | Suppress warnings and informational messages     |
| `-p`, `--pagesize SIZE`       | Specify page size in bytes (must be power of 2)  |
| `-L`, `--label LABEL`         | Volume label (max 16 bytes)                      |
| `-v`, `--swapversion NUM`     | Swap-space version (only version 1 is supported)  |
| `-U`, `--uuid UUID`           | UUID: `clear`, `random`, `time`, or explicit UUID|
| `-e`, `--endianness VALUE`    | Byte order: `native` (default), `little`, `big`   |
| `-o`, `--offset OFFSET`       | Byte offset of the swap header on the device     |
| `-s`, `--size SIZE`           | Swap file size with suffix (`K`, `M`, `G`, `T`, `P`)|
| `-F`, `--file`                | Create a regular file swap area                  |
| `--verbose`                   | Verbose output (currently a no-op placeholder)   |
| `--lock[=mode]`               | Exclusive device lock (`yes`, `no`, `nonblock`)  |
| `-V`, `--version`             | Display version and exit                         |
| `-h`, `--help`                | Display help and exit                            |

### Examples

```bash
# Basic swap on a block device
mkswap-rs /dev/sdb1

# Swap file, 4 GiB
mkswap-rs -F --size 4G /swapfile

# Explicit UUID and label
mkswap-rs -U a1b2c3d4-e5f6-7890-abcd-ef1234567890 -L my_swap /dev/sdb1

# Random UUID (default)
mkswap-rs -U random /dev/sdb1

# Clear UUID (all zeros — not recommended)
mkswap-rs -U clear /dev/sdb1

# Bad-block check with force flag
mkswap-rs -c -f /dev/sdc1

# Little-endian header at a 1 MiB offset
mkswap-rs -e little -o 1048576 /dev/sdb1

# Custom page size (must match kernel page size)
mkswap-rs -p 16384 /dev/sdb1

# Quiet mode for scripting
mkswap-rs -q /dev/sdb1

# Specify size in legacy block-count form (block = 1 KiB)
mkswap-rs /dev/sdb1 8388608
```

### Activation

After creating a swap area, activate it with `swapon`:

```bash
# Block device
swapon /dev/sdb1

# Swap file
swapon /swapfile

# Persistent activation (add to /etc/fstab)
echo "/dev/sdb1 none swap sw 0 0" >> /etc/fstab
echo "/swapfile none swap sw 0 0" >> /etc/fstab
```

Verification:

```bash
swapon --show
free -h
```

---

## Swap header format

The library produces a `swap_header_v1_2` structure starting at offset 1024
within the page:

```
Offset    Size   Field            Description
-------   ------ ---------------- ----------------------------------------------
0         1024   bootbits         Reserved for boot sector / disk label
1024      4      version          Swap version number (must be 1)
1028      4      last_page        Index of the last usable page
1032      4      nr_badpages      Number of entries in the bad-page array
1036      16     uuid             Partition UUID (RFC 4122 binary representation)
1052      16     volume_name      Partition label (null-padded)
1068      468    padding          117 × u32, alignment padding
1532      4+     badpages[]       Variable-length array of bad page numbers
```

The magic signature `SWAPSPACE2` is placed in the **last 10 bytes** of the page
(offsets `pagesize-10` through `pagesize-1`), not inside the 1024-byte header.

All multi-byte integer fields (`version`, `last_page`, `nr_badpages`, bad-page
entries) are written in the configured endianness.

---

## Comparison with `util-linux`'s `mkswap(8)`

| Feature                     | `mkswap(8)`    | `swap-rs` / `mkswap-rs` |
|-----------------------------|----------------|--------------------------|
| SWAPSPACE2 format           | Yes            | Yes                      |
| Bad-block checking (`-c`)   | Yes            | Yes                      |
| UUID / label                | Yes            | Yes                      |
| Offset (`-o`)               | Yes            | Yes                      |
| Size (`-s`)                 | Yes            | Yes                      |
| Swap file creation (`-F`)   | Yes            | Yes                      |
| Endianness selection (`-e`) | No (native)    | Native / Little / Big    |
| Library API                 | No (C source)  | Yes (Rust crate)         |
| Static musl build           | No             | Yes (`x86_64-unknown-linux-musl`) |

---

## Minimum supported Rust version (MSRV)

**Rust 1.70** or later.

---

## Building from source

```bash
git clone https://github.com/anomalyco/swap-rs.git
cd swap-rs

# Build the library
cargo build --release

# Build the CLI binary (if the binary feature is enabled)
cargo build --release --features cli
```

Static musl build:

```bash
rustup target add x86_64-unknown-linux-musl
cargo build --release --target x86_64-unknown-linux-musl
```

---

## Testing

```bash
cargo test
```

To run integration tests that require root privileges (use on a disposable
loopback device):

```bash
sudo cargo test -- --ignored
```

---

## License

Licensed under the [MIT License](LICENSE).

Copyright (c) 2026 Oleksandr Mametov
