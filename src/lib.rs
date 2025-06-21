#![no_std]

extern crate alloc;

pub mod block;
pub mod parse;
pub mod partition;

pub use crate::block::PartBlock;
pub use crate::partition::PartManager;

use alloc::{
    string::{String, ToString},
    vec::Vec,
};
use core::str::FromStr;
use log::{info, warn};

pub const BLOCK_SIZE: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartManError {
    /// Failed to read MBR
    MbrReadFailed,
    /// No Linux partition found
    NoLinuxPartition,
    /// Partition not found
    PartitionNotFound,
    /// Offset calculation overflow
    OffsetOverflow,
    /// Invalid data
    InvalidData,
    /// No root parameter
    NoRootParameter,
    /// Bootargs parse error
    BootargsParseError,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceType {
    Sata,
    SdMmc,
    Nvme,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartitionNaming {
    Direct,   // sda1, sda2
    Prefixed, // mmcblk0p1, mmcblk0p2
}

impl DeviceType {
    pub fn prefix(&self) -> &'static str {
        match self {
            DeviceType::Sata => "sd",
            DeviceType::SdMmc => "mmcblk",
            DeviceType::Nvme => "nvme",
        }
    }

    pub const fn partition_naming(&self) -> PartitionNaming {
        match self {
            Self::Sata => PartitionNaming::Direct,
            Self::SdMmc | Self::Nvme => PartitionNaming::Prefixed,
        }
    }
}

pub fn extract_root_from_bootargs(root_path: &str) -> Option<&str> {
    for part in root_path.split(' ') {
        if part.starts_with("root=") {
            return Some(&part[5..]);
        }
    }
    None
}

#[derive(Debug, Clone, PartialEq)]
pub enum RootDevice {
    DevicePath(String),
    Uuid(String),
    Label(String),
    PartUuid(String),
    MajorMinor(u32, u32),
}

#[derive(Debug, Clone, PartialEq)]
pub enum FileSystemType {
    Ext4,
    Ext3,
    Ext2,
    Xfs,
    Btrfs,
    F2fs,
    Ntfs,
    Vfat,
    Other(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct BootArgsFileSystem {
    pub root: Option<RootDevice>,
    pub rootfstype: Option<FileSystemType>,
    // ....
}

impl Default for BootArgsFileSystem {
    fn default() -> Self {
        Self {
            root: None,
            rootfstype: None,
        }
    }
}

impl FromStr for RootDevice {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.starts_with("UUID=") {
            Ok(RootDevice::Uuid(s[5..].to_string()))
        } else if s.starts_with("PARTLABEL=") {
            Ok(RootDevice::Label(s[10..].to_string()))
        } else if s.starts_with("PARTUUID=") {
            Ok(RootDevice::PartUuid(s[9..].to_string()))
        } else if s.starts_with("/dev/") {
            Ok(RootDevice::DevicePath(s.to_string()))
        } else if s.contains(':') {
            let parts: Vec<&str> = s.split(':').collect();
            if parts.len() == 2 {
                let major = parts[0]
                    .parse::<u32>()
                    .map_err(|_| "Invalid major number")?;
                let minor = parts[1]
                    .parse::<u32>()
                    .map_err(|_| "Invalid minor number")?;
                Ok(RootDevice::MajorMinor(major, minor))
            } else {
                Err("Invalid major:minor format".to_string())
            }
        } else {
            Ok(RootDevice::DevicePath(s.to_string()))
        }
    }
}

impl FromStr for FileSystemType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "ext4" => Ok(FileSystemType::Ext4),
            "ext3" => Ok(FileSystemType::Ext3),
            "ext2" => Ok(FileSystemType::Ext2),
            "xfs" => Ok(FileSystemType::Xfs),
            "btrfs" => Ok(FileSystemType::Btrfs),
            "f2fs" => Ok(FileSystemType::F2fs),
            "ntfs" => Ok(FileSystemType::Ntfs),
            "vfat" | "fat32" => Ok(FileSystemType::Vfat),
            _ => Ok(FileSystemType::Other(s.to_string())),
        }
    }
}

impl BootArgsFileSystem {
    pub fn new() -> Self {
        BootArgsFileSystem::default()
    }

    pub fn root_device(&self) -> Option<&RootDevice> {
        self.root.as_ref()
    }

    pub fn device_path(&self) -> Option<String> {
        self.root
            .as_ref()
            .and_then(|r| match r {
                RootDevice::DevicePath(path) => Some(path),
                RootDevice::Uuid(uuid) => Some(uuid),
                RootDevice::Label(label) => Some(label),
                RootDevice::PartUuid(partuuid) => Some(partuuid),
                RootDevice::MajorMinor(_, _) => None,
            })
            .cloned()
    }

    pub fn parse_from_bootargs(cmdline: &str) -> Result<Self, String> {
        let mut config = BootArgsFileSystem::default();

        for arg in cmdline.split_whitespace() {
            if let Some(value) = arg.strip_prefix("root=") {
                info!("Parsing root device: {}", value);
                config.root = Some(RootDevice::from_str(value)?);
            } else if let Some(value) = arg.strip_prefix("rootfstype=") {
                info!("Parsing root filesystem type: {}", value);
                config.rootfstype = Some(FileSystemType::from_str(value)?);
            } else {
                warn!("Unknown boot argument: {}", arg);
            }
        }

        Ok(config)
    }
}
