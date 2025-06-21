use alloc::{fmt, vec::Vec};
use gpt_parser::{GPTHeader, GPTPartition, LBA, LBA512};
use log::{debug, info, warn};
use mbrs::{Mbr, PartInfo, PartType};

use crate::parse::{
    parse_device_label, parse_device_partuuid, parse_device_path, parse_device_uuid,
    parse_major_minor,
};
use crate::{BLOCK_SIZE, BootArgsFileSystem, DeviceType, PartBlock, PartManError, RootDevice};

#[derive(Debug, Clone)]
pub enum PartitionTableType {
    MBR,
    GPT,
}

/// Unified partition manager
/// Supports MBR and GPT partition tables
pub struct PartManager {
    table_type: PartitionTableType,
    mbr: Option<Mbr>,
    gpt_header: Option<GPTHeader>,
    gpt_partitions: Vec<GPTPartition>,
    rootargs: BootArgsFileSystem,
}

impl PartManager {
    /// Creates a new partition manager instance
    /// Automatically detects partition table type (GPT or MBR)
    pub fn new<T: PartBlock>(
        dev: &mut T,
        rootargs: BootArgsFileSystem,
    ) -> Result<Self, &'static str> {
        debug!("Starting partition manager initialization");

        let mut mbr_data = [0u8; 512];
        dev.read_block(0, &mut mbr_data)
            .map_err(|_| "Failed to read MBR")?;

        if mbr_data[510] != 0x55 || mbr_data[511] != 0xAA {
            return Err("Invalid MBR signature");
        }

        if is_protective_mbr(&mbr_data) {
            info!("Detected GPT partition table");
            Self::new_gpt(dev, rootargs)
        } else {
            info!("Detected MBR partition table");
            Self::new_mbr(dev, mbr_data, rootargs)
        }
    }

    /// Creates a GPT partition manager
    fn new_gpt<T: PartBlock>(
        dev: &mut T,
        rootargs: BootArgsFileSystem,
    ) -> Result<Self, &'static str> {
        let gpt_header = Self::verify_and_read_gpt(dev)?;

        let partitions = Self::read_partition_table(dev, &gpt_header)?;

        info!(
            "GPT manager initialized successfully, found {} valid partitions",
            partitions.len()
        );

        Ok(Self {
            table_type: PartitionTableType::GPT,
            mbr: None,
            gpt_header: Some(gpt_header),
            gpt_partitions: partitions,
            rootargs,
        })
    }

    /// Creates an MBR partition manager
    fn new_mbr<T: PartBlock>(
        _dev: &mut T,
        mbr_data: [u8; 512],
        rootargs: BootArgsFileSystem,
    ) -> Result<Self, &'static str> {
        let mbr = Mbr::try_from_bytes(&mbr_data).map_err(|_| "Invalid MBR data")?;

        info!("MBR manager initialized successfully");

        Ok(Self {
            table_type: PartitionTableType::MBR,
            mbr: Some(mbr),
            gpt_header: None,
            gpt_partitions: Vec::new(),
            rootargs,
        })
    }

    /// Returns the partition table type
    pub fn table_type(&self) -> &PartitionTableType {
        &self.table_type
    }

    /// Returns boot arguments
    pub fn rootargs(&self) -> &BootArgsFileSystem {
        &self.rootargs
    }

    /// Returns MBR (valid only if the table type is MBR)
    pub fn mbr(&self) -> Option<&Mbr> {
        self.mbr.as_ref()
    }

    /// Returns GPT header (valid only if the table type is GPT)
    pub fn gpt_header(&self) -> Option<&GPTHeader> {
        self.gpt_header.as_ref()
    }

    /// Returns GPT partition list (valid only if the table type is GPT)
    pub fn gpt_partitions(&self) -> &[GPTPartition] {
        &self.gpt_partitions
    }

    /// Calculates the partition offset
    pub fn part_offset(&mut self) -> Result<usize, &'static str> {
        match self.table_type {
            PartitionTableType::GPT => self.gpt_part_offset(),
            PartitionTableType::MBR => self.mbr_part_offset(),
        }
    }

    /// GPT partition offset calculation
    fn gpt_part_offset(&mut self) -> Result<usize, &'static str> {
        let first_lba: Option<u64> = self.rootargs().root_device().and_then(|r| match r {
            RootDevice::Label(label) => self.parse_gpt_device_label(label),
            _ => panic!("Unsupported root device type for partition offset"),
        });

        if let Some(lba) = first_lba {
            let offset = lba as usize * BLOCK_SIZE;
            debug!("Calculated GPT partition offset: {:x?}", offset);
            Ok(offset)
        } else {
            warn!("Failed to parse root device label for GPT partition offset");
            Err("Failed to parse root device label for GPT partition offset")
        }
    }

    /// MBR partition offset calculation
    fn mbr_part_offset(&mut self) -> Result<usize, &'static str> {
        match self.mbr_calculate_offset() {
            Ok(partition) => {
                let start_sector = partition.part_info().start_sector_lba() as usize * BLOCK_SIZE;
                Ok(start_sector)
            }
            Err(_) => Err("No suitable MBR partition found"),
        }
    }

    /// Looks up GPT partition by label
    fn parse_gpt_device_label(&self, label: &str) -> Option<u64> {
        info!("Parsing GPT device label: {}", label);

        self.find_gpt_partition_by_name(label)
            .map(|part| part.first_lba.into())
    }

    /// Finds GPT partition by name
    pub fn find_gpt_partition_by_name(&self, name: &str) -> Option<&GPTPartition> {
        self.gpt_partitions
            .iter()
            .find(|part| part.get_name().map_or(false, |part_name| part_name == name))
    }

    /// Finds GPT partitions by type UUID
    pub fn find_gpt_partitions_by_type(&self, part_type: &gpt_parser::Uuid) -> Vec<&GPTPartition> {
        self.gpt_partitions
            .iter()
            .filter(|part| part.part_type == *part_type)
            .collect()
    }

    /// Returns the EFI system partition
    pub fn get_efi_system_partition(&self) -> Option<&GPTPartition> {
        let efi_uuid = gpt_parser::Uuid::efi();
        self.find_gpt_partitions_by_type(&efi_uuid)
            .into_iter()
            .next()
    }

    /// MBR offset calculation logic
    fn mbr_calculate_offset(&self) -> Result<MbrPartition, PartManError> {
        match self.mbr_offset_from_rootargs() {
            Ok(offset) => {
                info!("Successfully calculated MBR offset from root: {:?}", offset);
                return Ok(offset);
            }
            Err(e) => {
                info!(
                    "Failed to parse MBR root: {:?}, falling back to MBR analysis",
                    e
                );
            }
        }

        self.mbr_smart_guess_offset()
    }

    /// Calculates MBR offset from boot arguments
    fn mbr_offset_from_rootargs(&self) -> Result<MbrPartition, PartManError> {
        let (device_type, device_index, partition_num) = self
            .parse_root_device()
            .ok_or(PartManError::BootargsParseError)?;
        info!(
            "Parsed MBR device: type={:?}, index={}, partition={}",
            device_type, device_index, partition_num
        );

        self.get_mbr_partition_offset_by_number(partition_num)
    }

    /// Smart guessing MBR offset
    fn mbr_smart_guess_offset(&self) -> Result<MbrPartition, PartManError> {
        let mbr = self.mbr.as_ref().ok_or(PartManError::NoLinuxPartition)?;

        let partition_priority = |part_type: &PartType| -> u8 {
            match part_type {
                PartType::LinuxNative => 1, // 最高优先级
                PartType::LinuxLvm => 2,
                PartType::LinuxSwap => 3,
                _ => 255, // 非 Linux 分区
            }
        };

        let best_partition = mbr
            .partition_table
            .entries
            .iter()
            .enumerate()
            .filter_map(|(idx, entry)| entry.as_ref().map(|e| (idx, e)))
            .filter(|(_, partinfo)| self.is_linux_partition(partinfo))
            .min_by_key(|(_, partinfo)| partition_priority(partinfo.part_type()));

        match best_partition {
            Some((idx, partinfo)) => {
                let offset = self.create_mbr_partition_offset(idx as u8 + 1, partinfo)?;
                info!("Smart guess selected MBR partition: {:?}", offset);
                Ok(offset)
            }
            None => Err(PartManError::NoLinuxPartition),
        }
    }

    /// Gets MBR partition offset by partition number
    fn get_mbr_partition_offset_by_number(
        &self,
        partition_num: u8,
    ) -> Result<MbrPartition, PartManError> {
        if partition_num == 0 || partition_num > 4 {
            return Err(PartManError::PartitionNotFound);
        }

        let mbr = self.mbr.as_ref().ok_or(PartManError::NoLinuxPartition)?;
        let idx = (partition_num - 1) as usize;

        if let Some(partinfo) = &mbr.partition_table.entries[idx] {
            self.create_mbr_partition_offset(partition_num, partinfo)
        } else {
            Err(PartManError::PartitionNotFound)
        }
    }

    /// Creates an MBR partition offset object
    fn create_mbr_partition_offset(
        &self,
        partition_index: u8,
        partinfo: &PartInfo,
    ) -> Result<MbrPartition, PartManError> {
        let mbr_partition = MbrPartition::new(partition_index, partinfo.clone());
        Ok(mbr_partition)
    }

    /// Checks whether the partition is a Linux partition
    fn is_linux_partition(&self, partinfo: &PartInfo) -> bool {
        match partinfo.part_type() {
            PartType::LinuxNative |     // 0x83
            PartType::LinuxSwap |       // 0x82  
            PartType::LinuxLvm => true, // 0x8E
            _ => false,
        }
    }

    /// Gets all valid MBR partitions
    pub fn get_all_mbr_partitions(&self) -> Vec<MbrPartition> {
        let mut partitions = Vec::new();

        if let Some(mbr) = &self.mbr {
            for (idx, entry) in mbr.partition_table.entries.iter().enumerate() {
                if let Some(partinfo) = entry {
                    if let Ok(offset) = self.create_mbr_partition_offset(idx as u8 + 1, partinfo) {
                        partitions.push(offset);
                    }
                }
            }
        }

        partitions
    }

    /// Checks if the partition table is valid
    pub fn is_valid(&self) -> bool {
        match self.table_type {
            PartitionTableType::GPT => !self.gpt_partitions.is_empty(),
            PartitionTableType::MBR => self
                .mbr
                .as_ref()
                .map_or(false, |mbr| mbr.bootsector_signature == [0x55, 0xAA]),
        }
    }

    /// Gets disk info (GPT only)
    pub fn get_disk_info(&self) -> Option<DiskInfo> {
        match self.table_type {
            PartitionTableType::GPT => {
                let gpt_header = self.gpt_header.as_ref()?;
                let first_usable = Into::<u64>::into(gpt_header.first_usable);
                let last_usable = Into::<u64>::into(gpt_header.last_usable);
                let total_usable_sectors = last_usable - first_usable + 1;

                let mut used_sectors = 0u64;
                for partition in &self.gpt_partitions {
                    let start = Into::<u64>::into(partition.first_lba);
                    let end = Into::<u64>::into(partition.last_lba);
                    used_sectors += end - start + 1;
                }

                Some(DiskInfo {
                    first_usable_lba: first_usable,
                    last_usable_lba: last_usable,
                    total_usable_sectors,
                    used_sectors,
                    free_sectors: total_usable_sectors.saturating_sub(used_sectors),
                    partition_count: self.gpt_partitions.len(),
                })
            }
            PartitionTableType::MBR => None,
        }
    }

    /// Parses root device
    fn parse_root_device(&self) -> Option<(DeviceType, u8, u8)> {
        self.rootargs().root_device().and_then(|r| match r {
            RootDevice::DevicePath(path) => parse_device_path(&path),
            RootDevice::Uuid(uuid) => parse_device_uuid(uuid),
            RootDevice::Label(label) => parse_device_label(label),
            RootDevice::PartUuid(partuuid) => parse_device_partuuid(partuuid),
            RootDevice::MajorMinor(major, minor) => parse_major_minor(major, minor),
        })
    }

    // GPT-related helper methods
    fn verify_and_read_gpt<T: PartBlock>(dev: &mut T) -> Result<GPTHeader, &'static str> {
        // 读取 GPT 头 (LBA 1)
        let mut gpt_data = [0u8; 512];
        dev.read_block(1, &mut gpt_data)
            .map_err(|_| "Failed to read GPT header")?;

        let gpt_lba = gpt_data.as_ptr() as *const LBA<LBA512>;
        let gpt_header = unsafe { GPTHeader::parse_gpt_header(&*gpt_lba) }.map_err(|e| {
            warn!("Failed to parse GPT header: {}", e);
            "Invalid GPT header data"
        })?;

        debug!("GPT header parsed successfully");
        debug!(
            "Partition table start LBA: {}",
            Into::<u64>::into(gpt_header.part_start_lba)
        );
        debug!(
            "Number of partition entries: {}",
            Into::<u32>::into(gpt_header.num_parts)
        );
        debug!(
            "Partition entry size: {} bytes",
            Into::<u32>::into(gpt_header.part_size)
        );

        Ok(*gpt_header)
    }

    fn read_partition_table<T: PartBlock>(
        dev: &mut T,
        gpt_header: &GPTHeader,
    ) -> Result<Vec<GPTPartition>, &'static str> {
        let partition_start_lba = Into::<u64>::into(gpt_header.part_start_lba);
        let part_size = Into::<u32>::into(gpt_header.part_size) as usize;
        let num_parts = Into::<u32>::into(gpt_header.num_parts) as usize;

        let total_partition_bytes = num_parts * part_size;
        let lba_blocks_needed = (total_partition_bytes + 511) / 512;

        debug!(
            "Need to read {} LBA blocks to get complete partition table",
            lba_blocks_needed
        );

        let partition_data =
            Self::read_partition_data(dev, partition_start_lba, lba_blocks_needed)?;

        Self::parse_partitions(&partition_data, gpt_header)
    }

    fn read_partition_data<T: PartBlock>(
        dev: &mut T,
        start_lba: u64,
        block_count: usize,
    ) -> Result<Vec<u8>, &'static str> {
        let mut data = Vec::with_capacity(block_count * 512);
        data.resize(block_count * 512, 0);

        for i in 0..block_count {
            let lba = start_lba + i as u64;
            let offset = i * 512;

            dev.read_block(lba, &mut data[offset..offset + 512])
                .map_err(|_| {
                    warn!("Failed to read LBA {}", lba);
                    "Failed to read partition table data"
                })?;
        }

        debug!(
            "Successfully read {} bytes of partition table data",
            data.len()
        );
        Ok(data)
    }

    fn parse_partitions(
        partition_data: &[u8],
        gpt_header: &GPTHeader,
    ) -> Result<Vec<GPTPartition>, &'static str> {
        let part_size = Into::<u32>::into(gpt_header.part_size) as usize;
        let num_parts = Into::<u32>::into(gpt_header.num_parts) as usize;

        let partition_ptr = partition_data.as_ptr() as *const GPTPartition;
        let stride = part_size / core::mem::size_of::<GPTPartition>();
        let length = stride * num_parts;

        let part_array = unsafe {
            let ptr = core::ptr::slice_from_raw_parts(partition_ptr, length);
            &*ptr
        };

        let mut partitions = Vec::new();
        GPTHeader::parse_partitions(
            part_array,
            part_size,
            |part, parts: &mut Vec<GPTPartition>| {
                parts.push(*part);
            },
            &mut partitions,
        );

        for (i, partition) in partitions.iter().enumerate() {
            if let Ok(name) = partition.get_name() {
                let start_lba = Into::<u64>::into(partition.first_lba);
                let end_lba = Into::<u64>::into(partition.last_lba);
                let size_sectors = end_lba - start_lba + 1;
                let size_mb = (size_sectors * 512) / 1024 / 1024;

                info!(
                    "Partition {}: '{}' (LBA {}-{}, {} MB)",
                    i, name, start_lba, end_lba, size_mb
                );
            } else {
                debug!("Partition {} name parsing failed", i);
            }
        }

        Ok(partitions)
    }
}

/// Disk information structure (GPT only)
#[derive(Debug, Clone)]
pub struct DiskInfo {
    pub first_usable_lba: u64,
    pub last_usable_lba: u64,
    pub total_usable_sectors: u64,
    pub used_sectors: u64,
    pub free_sectors: u64,
    pub partition_count: usize,
}

impl DiskInfo {
    /// Returns total usable space in bytes
    pub fn total_usable_bytes(&self) -> u64 {
        self.total_usable_sectors * 512
    }

    /// Returns used space in bytes
    pub fn used_bytes(&self) -> u64 {
        self.used_sectors * 512
    }

    /// Returns free space in bytes
    pub fn free_bytes(&self) -> u64 {
        self.free_sectors * 512
    }

    /// Returns usage percentage
    pub fn usage_percentage(&self) -> f32 {
        if self.total_usable_sectors == 0 {
            0.0
        } else {
            (self.used_sectors as f32 / self.total_usable_sectors as f32) * 100.0
        }
    }
}

/// Checks whether the MBR is a protective MBR (indicating GPT)
fn is_protective_mbr(data: &[u8; 512]) -> bool {
    let mut has_gpt_protective = false;
    let mut non_zero_partitions = 0;

    for i in 0..4 {
        let offset = 446 + i * 16;
        let partition_type = data[offset + 4];

        if partition_type != 0 {
            non_zero_partitions += 1;
        }

        if partition_type == 0xEE {
            has_gpt_protective = true;
            debug!("Found GPT protective partition (entry {})", i);
        }
    }

    if !has_gpt_protective {
        debug!("No GPT protective partition (type 0xEE) found");
    }

    if non_zero_partitions > 1 && has_gpt_protective {
        warn!(
            "Mixed MBR/GPT configuration detected: {} non-zero partitions",
            non_zero_partitions
        );
    }

    has_gpt_protective
}

/// MBR partition structure
pub struct MbrPartition {
    partition_index: u8,
    part_info: PartInfo,
}

impl MbrPartition {
    pub fn new(partition_index: u8, part_info: PartInfo) -> Self {
        MbrPartition {
            partition_index,
            part_info,
        }
    }

    pub fn partition_index(&self) -> u8 {
        self.partition_index
    }

    pub fn part_info(&self) -> &PartInfo {
        &self.part_info
    }
}

impl fmt::Debug for MbrPartition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "MbrPartition(index={}, type={:?}, start={}, size={})",
            self.partition_index,
            self.part_info.part_type(),
            self.part_info.start_sector_lba(),
            self.part_info.sector_count_lba()
        )
    }
}
