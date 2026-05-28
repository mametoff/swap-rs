use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::unix::fs::OpenOptionsExt;

/// Linux swap-space version implemented by this library.
pub const SWAP_VERSION: u32 = 1;

/// Length of the UUID field in the swap header (bytes).
pub const SWAP_UUID_LENGTH: usize = 16;

/// Maximum length of the volume label (bytes).
pub const SWAP_LABEL_LENGTH: usize = 16;

/// The magic signature identifying a Linux swap area (SWAPSPACE2).
pub const SWAP_SIGNATURE: &[u8] = b"SWAPSPACE2";

/// Size of the SWAPSPACE2 signature in bytes.
pub const SWAP_SIGNATURE_SZ: usize = 10;

/// Minimum number of good pages required for a valid swap area.
pub const MIN_GOODPAGES: u64 = 10;

/// Byte offset of the swap header version field within the page.
pub const SIGNATURE_OFFSET: u64 = 1024;

/// Raw on-disk swap header structure (version 1, layout compatible with Linux 2.4+).
///
/// # Layout
///
/// | Offset | Size | Field         | Description                     |
/// |--------|------|---------------|---------------------------------|
/// | 0      | 1024 | `bootbits`    | Boot sector / disk label        |
/// | 1024   | 4    | `version`     | Swap version (must be 1)        |
/// | 1028   | 4    | `last_page`   | Last usable page number         |
/// | 1032   | 4    | `nr_badpages` | Number of bad pages recorded    |
/// | 1036   | 16   | `uuid`        | Partition UUID                  |
/// | 1052   | 16   | `volume_name` | Partition label                 |
/// | 1068   | 468  | `padding`     | Padding (117 × u32)             |
/// | 1532   | 4+   | `badpages`    | Bad page numbers (variable)     |
///
/// The `SWAPSPACE2` signature is written in the **last 10 bytes** of the page,
/// not inside this structure.
#[repr(C, packed)]
pub struct SwapHeaderV12 {
    pub bootbits: [u8; 1024],
    pub version: u32,
    pub last_page: u32,
    pub nr_badpages: u32,
    pub uuid: [u8; SWAP_UUID_LENGTH],
    pub volume_name: [u8; SWAP_LABEL_LENGTH],
    pub padding: [u32; 117],
    pub badpages: [u32; 1],
}

/// Byte order used for multi-byte fields in the swap header.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Endianness {
    Native,
    Little,
    Big,
}

/// Configuration for creating a Linux swap area.
///
/// Pass an instance of this struct to [`mkswap`] to create the swap area.
///
/// # Example
///
/// ```no_run
/// use swap_rs::{MkswapConfig, Endianness};
///
/// let config = MkswapConfig {
///     device: "/dev/sdb1".into(),
///     label: Some("swap".into()),
///     uuid: None,
///     endianness: Endianness::Native,
///     ..Default::default()
/// };
/// swap_rs::mkswap(&config).unwrap();
/// ```
#[derive(Clone, Debug)]
pub struct MkswapConfig {
    /// Path to the block device or file.
    pub device: String,
    /// Number of pages (derived from `size_in_blocks` CLI arg). 0 = auto.
    pub npages: u64,
    /// File size in bytes (for `--size` / `-s`).
    pub filesz: u64,
    /// Treat the device as a regular file.
    pub file: bool,
    /// Byte offset into the device where the swap header is placed.
    pub offset: u64,
    /// Optional volume label (max 16 bytes).
    pub label: Option<String>,
    /// Optional UUID bytes. If `None`, a random UUID is generated.
    pub uuid: Option<[u8; SWAP_UUID_LENGTH]>,
    /// Byte order for multi-word fields.
    pub endianness: Endianness,
    /// User-specified page size in bytes (0 = use system page size).
    pub user_pagesize: i32,
    /// Perform bad-block checking.
    pub check: bool,
    /// Skip boot-bits erasure and allow size > device.
    pub force: bool,
    /// Suppress informational messages.
    pub quiet: bool,
}

impl Default for MkswapConfig {
    fn default() -> Self {
        Self {
            device: String::new(),
            npages: 0,
            filesz: 0,
            file: false,
            offset: 0,
            label: None,
            uuid: None,
            endianness: Endianness::Native,
            user_pagesize: 0,
            check: false,
            force: false,
            quiet: false,
        }
    }
}

/// Error type for swap-rs operations.
#[derive(Debug)]
pub enum SwapError {
    Io(std::io::Error),
    Msg(String),
}

impl std::fmt::Display for SwapError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SwapError::Io(e) => write!(f, "{}", e),
            SwapError::Msg(s) => write!(f, "{}", s),
        }
    }
}

impl std::error::Error for SwapError {}

impl From<std::io::Error> for SwapError {
    fn from(e: std::io::Error) -> Self {
        SwapError::Io(e)
    }
}

impl From<String> for SwapError {
    fn from(s: String) -> Self {
        SwapError::Msg(s)
    }
}

impl From<&str> for SwapError {
    fn from(s: &str) -> Self {
        SwapError::Msg(s.to_string())
    }
}

/// Convert a `u32` value to the target byte order.
pub fn cpu32_to_endianness(v: u32, e: Endianness) -> u32 {
    match e {
        Endianness::Native => v,
        Endianness::Little => v.to_le(),
        Endianness::Big => v.to_be(),
    }
}

/// Return `true` if `n` is a power of two.
pub fn is_power_of_2(n: usize) -> bool {
    n != 0 && (n & (n - 1)) == 0
}

/// Allocate and zero a page-sized buffer, validating user-supplied page size.
///
/// Returns the zeroed buffer and updates `pagesize` to the resolved value.
pub fn init_signature_page(
    pagesize: &mut usize,
    user_pagesize: i32,
    quiet: bool,
) -> Result<Vec<u8>, SwapError> {
    if user_pagesize != 0 {
        let ps = user_pagesize as usize;
        if user_pagesize < 0
            || !is_power_of_2(ps)
            || ps < std::mem::size_of::<SwapHeaderV12>() + 10
        {
            return Err(format!("Bad user-specified page size {}", user_pagesize).into());
        }
        if !quiet && ps != page_size() {
            eprintln!(
                "Using user-specified page size {}, instead of the system value {}",
                ps,
                page_size()
            );
        }
        *pagesize = ps;
    } else {
        *pagesize = page_size();
    }
    Ok(vec![0u8; *pagesize])
}

/// Write the `SWAPSPACE2` signature to the last 10 bytes of the page.
pub fn set_signature(signature_page: &mut [u8], pagesize: usize) {
    let offset = pagesize - SWAP_SIGNATURE_SZ;
    signature_page[offset..offset + SWAP_SIGNATURE_SZ].copy_from_slice(SWAP_SIGNATURE);
}

/// Record a bad page number in the swap header's bad-page array.
pub fn page_bad(
    signature_page: &mut [u8],
    nbadpages: &mut u32,
    pagesize: usize,
    endianness: Endianness,
    page: u32,
) -> Result<(), SwapError> {
    let max_badpages =
        (pagesize - 1024 - 128 * std::mem::size_of::<u32>() - 10) / std::mem::size_of::<u32>();
    if *nbadpages as usize == max_badpages {
        return Err(format!("too many bad pages: {}", max_badpages).into());
    }
    let offset = std::mem::size_of::<SwapHeaderV12>() - std::mem::size_of::<u32>()
        + (*nbadpages as usize) * std::mem::size_of::<u32>();
    let slice = &mut signature_page[offset..offset + 4];
    slice.copy_from_slice(&cpu32_to_endianness(page, endianness).to_ne_bytes());
    *nbadpages += 1;
    Ok(())
}

/// Read every page on the device to detect I/O errors (bad blocks).
pub fn check_blocks(
    fd: &File,
    npages: u64,
    pagesize: usize,
    nbadpages: &mut u32,
    signature_page: &mut [u8],
    endianness: Endianness,
    quiet: bool,
) -> Result<(), SwapError> {
    let mut current_page: u32 = 0;
    let mut buffer = vec![0u8; pagesize];
    let mut file = fd.try_clone()?;

    while (current_page as u64) < npages {
        let offset = (current_page as u64) * pagesize as u64;
        file.seek(SeekFrom::Start(offset))
            .map_err(|_| "seek failed in check_blocks")?;
        let mut reader = std::io::Read::by_ref(&mut file).take(pagesize as u64);
        let rc = reader.read(&mut buffer);
        match rc {
            Ok(n) if n == pagesize => {}
            _ => {
                page_bad(
                    signature_page,
                    nbadpages,
                    pagesize,
                    endianness,
                    current_page,
                )?;
            }
        }
        current_page += 1;
    }

    if !quiet {
        println!(
            "{} bad page{}",
            nbadpages,
            if *nbadpages == 1 { "" } else { "s" }
        );
    }
    Ok(())
}

/// Determine the usable size of a device or file.
pub fn get_size(devname: &str, offset: u64, file: bool, filesz: u64) -> Result<u64, SwapError> {
    if file && filesz > 0 {
        Ok(filesz)
    } else {
        let f = File::open(devname)?;
        let size = f.metadata()?.len();
        if offset > size {
            return Err("offset larger than file size".into());
        }
        Ok(size - offset)
    }
}

/// Open a device or create a swap file.
pub fn open_device(
    devname: &str,
    file: bool,
    filesz: u64,
    _check: &mut bool,
    _quiet: bool,
) -> Result<(Option<fs::Metadata>, File), SwapError> {
    if file {
        if let Ok(stat) = fs::metadata(devname) {
            if !stat.file_type().is_file() {
                return Err(format!(
                    "cannot create swap file {}: node isn't regular file",
                    devname
                )
                .into());
            }
        }
        let fd = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .mode(0o600)
            .open(devname)?;
        if filesz > 0 {
            fd.set_len(filesz)?;
        }
        Ok((fs::metadata(devname).ok(), fd))
    } else {
        let stat = fs::metadata(devname)?;
        let fd = OpenOptions::new()
            .read(true)
            .write(true)
            .open(devname)?;
        Ok((Some(stat), fd))
    }
}

/// Zero the first 1024 bytes of the device (boot bits) unless `force` is set.
pub fn wipe_device(fd: &File, devname: &str, force: bool, quiet: bool) -> Result<(), SwapError> {
    if !force {
        let mut f = fd.try_clone()?;
        f.seek(SeekFrom::Start(0))?;
        let buf = vec![0u8; 1024];
        f.write_all(&buf)?;
    } else if !quiet {
        eprintln!("{}: warning: don't erase bootbits sectors", devname);
        eprintln!("        Use -f to force.");
    }
    Ok(())
}

/// Write the signature page (offset 1024..pagesize) to the device.
pub fn write_header_to_device(
    fd: &File,
    signature_page: &[u8],
    offset: u64,
    pagesize: usize,
) -> Result<(), SwapError> {
    let mut f = fd.try_clone()?;
    let woffset = SIGNATURE_OFFSET + offset;

    f.seek(SeekFrom::Start(woffset))?;

    let data = &signature_page[SIGNATURE_OFFSET as usize..pagesize];
    f.write_all(data)?;
    Ok(())
}

/// Parse a size string with optional K/M/G/T/P suffix into bytes.
///
/// # Examples
///
/// ```
/// assert_eq!(swap_rs::parse_size("4K").unwrap(), 4096);
/// assert_eq!(swap_rs::parse_size("1M").unwrap(), 1048576);
/// assert_eq!(swap_rs::parse_size("512").unwrap(), 512);
/// ```
pub fn parse_size(s: &str) -> Result<u64, SwapError> {
    let s = s.trim();
    let (num_str, multiplier) = if s.ends_with('K') || s.ends_with('k') {
        (&s[..s.len() - 1], 1024u64)
    } else if s.ends_with('M') || s.ends_with('m') {
        (&s[..s.len() - 1], 1024u64 * 1024)
    } else if s.ends_with('G') || s.ends_with('g') {
        (&s[..s.len() - 1], 1024u64 * 1024 * 1024)
    } else if s.ends_with('T') || s.ends_with('t') {
        (&s[..s.len() - 1], 1024u64 * 1024 * 1024 * 1024)
    } else if s.ends_with('P') || s.ends_with('p') {
        (&s[..s.len() - 1], 1024u64 * 1024 * 1024 * 1024 * 1024)
    } else {
        (s, 1u64)
    };
    let num: u64 = num_str
        .parse()
        .map_err(|_| format!("invalid size: '{}'", s))?;
    Ok(num * multiplier)
}

/// Return the system page size in bytes (via `sysconf(_SC_PAGESIZE)`).
pub fn page_size() -> usize {
    unsafe { libc::sysconf(libc::_SC_PAGESIZE) as usize }
}

/// Cast a byte slice to a mutable `SwapHeaderV12` reference.
///
/// # Panics
///
/// Panics if `sig` is shorter than `size_of::<SwapHeaderV12>()`.
pub fn hdr_mut(sig: &mut [u8]) -> &mut SwapHeaderV12 {
    assert!(sig.len() >= std::mem::size_of::<SwapHeaderV12>());
    unsafe { &mut *(sig.as_mut_ptr() as *mut SwapHeaderV12) }
}

/// Create a complete Linux swap area on the given device or file.
///
/// This is the primary high-level API. It performs all steps:
///
/// 1. Resolves page size
/// 2. Validates device size and page count
/// 3. Opens the device (or creates a swap file)
/// 4. Optionally checks for bad blocks
/// 5. Wipes boot bits (unless `force`)
/// 6. Constructs the swap header (version, UUID, label, endianness)
/// 7. Writes the `SWAPSPACE2` signature
/// 8. Writes the header to device and syncs
///
/// # Example
///
/// ```no_run
/// use swap_rs::{MkswapConfig, Endianness, mkswap};
///
/// let config = MkswapConfig {
///     device: "/swapfile".into(),
///     file: true,
///     filesz: 1024 * 1024 * 1024, // 1 GiB
///     label: Some("swap".into()),
///     ..Default::default()
/// };
/// mkswap(&config).unwrap();
/// ```
pub fn mkswap(config: &MkswapConfig) -> Result<(), SwapError> {
    let mut pagesize: usize = 0;
    let mut nbadpages: u32 = 0;
    let uuid = config.uuid.unwrap_or_else(|| *uuid::Uuid::new_v4().as_bytes());

    let mut signature_page =
        init_signature_page(&mut pagesize, config.user_pagesize, config.quiet)?;

    let sz = get_size(&config.device, config.offset, config.file, config.filesz)?;
    let mut npages = config.npages;
    if npages == 0 {
        npages = sz / pagesize as u64;
    } else if npages > sz / pagesize as u64 && !config.force {
        return Err(format!(
            "error: size {} KiB is larger than device size {} KiB",
            npages * pagesize as u64 / 1024,
            sz / 1024
        )
        .into());
    }

    if npages < MIN_GOODPAGES {
        return Err(format!(
            "error: swap area needs to be at least {} KiB",
            MIN_GOODPAGES * pagesize as u64 / 1024
        )
        .into());
    }

    let npages = if npages > u32::MAX as u64 {
        if !config.quiet {
            eprintln!(
                "warning: truncating swap area to {} KiB",
                u32::MAX as u64 * pagesize as u64 / 1024
            );
        }
        u32::MAX as u64
    } else {
        npages
    };

    let mut check = config.check;
    let (_devstat, fd) = open_device(
        &config.device,
        config.file,
        config.filesz,
        &mut check,
        config.quiet,
    )?;

    if check {
        check_blocks(
            &fd,
            npages,
            pagesize,
            &mut nbadpages,
            &mut signature_page,
            config.endianness,
            config.quiet,
        )?;
    }

    wipe_device(&fd, &config.device, !config.force, config.quiet)?;

    {
        let h = hdr_mut(&mut signature_page);
        h.version = cpu32_to_endianness(SWAP_VERSION, config.endianness);
        h.last_page = cpu32_to_endianness((npages - 1) as u32, config.endianness);
        h.nr_badpages = cpu32_to_endianness(nbadpages, config.endianness);
    }

    if (npages - MIN_GOODPAGES) < nbadpages as u64 {
        return Err("Unable to set up swap-space: unreadable".into());
    }

    let sz_bytes = (npages - nbadpages as u64 - 1) * pagesize as u64;
    if !config.quiet {
        println!(
            "Setting up swapspace version {}, size = {} bytes",
            SWAP_VERSION, sz_bytes
        );
    }

    set_signature(&mut signature_page, pagesize);

    if config.label.is_some() {
        let h = hdr_mut(&mut signature_page);
        h.uuid.copy_from_slice(&uuid);
        if let Some(ref label) = config.label {
            let label_bytes = label.as_bytes();
            let len = std::cmp::min(label_bytes.len(), SWAP_LABEL_LENGTH);
            h.volume_name[..len].copy_from_slice(&label_bytes[..len]);
            if len < SWAP_LABEL_LENGTH {
                h.volume_name[len..].fill(0);
            }
            if !config.quiet && label_bytes.len() > SWAP_LABEL_LENGTH {
                eprintln!("Label was truncated.");
            }
        }
        if !config.quiet {
            if let Some(ref label) = config.label {
                print!(
                    "LABEL={}, ",
                    &label[..std::cmp::min(label.len(), SWAP_LABEL_LENGTH)]
                );
            } else {
                print!("no label, ");
            }
            println!("UUID={}", uuid::Uuid::from_bytes(uuid));
        }
    } else {
        let h = hdr_mut(&mut signature_page);
        h.uuid.copy_from_slice(&uuid);
        if !config.quiet {
            println!("UUID={}", uuid::Uuid::from_bytes(uuid));
        }
    }

    write_header_to_device(&fd, &signature_page, config.offset, pagesize)?;
    fd.sync_all()?;

    Ok(())
}
