use crate::DeviceType;
use log::info;

pub fn parse_device_path(path: &str) -> Option<(DeviceType, u8, u8)> {
    if !path.starts_with("/dev/") {
        return None;
    }

    let device_part = &path[5..];

    if device_part.len() >= 4 && device_part.starts_with("sd") {
        let device_char = device_part.chars().nth(2)?;
        let partition_char = device_part.chars().nth(3)?;

        if device_char >= 'a'
            && device_char <= 'z'
            && partition_char >= '1'
            && partition_char <= '9'
        {
            let device_index = (device_char as u8) - b'a';
            let partition = (partition_char as u8) - b'0';
            info!(
                "Parsed SATA device: index={}, partition={}",
                device_index, partition
            );
            return Some((DeviceType::Sata, device_index, partition));
        }
    }

    if device_part.starts_with("mmcblk") && device_part.len() >= 9 {
        if let Some(p_pos) = device_part.find('p') {
            let device_char = device_part.chars().nth(6)?; // number after mmcblk
            let partition_char = device_part.chars().nth(p_pos + 1)?;

            if device_char >= '0'
                && device_char <= '9'
                && partition_char >= '1'
                && partition_char <= '9'
            {
                let device_index = (device_char as u8) - b'0';
                let partition = (partition_char as u8) - b'0';
                info!(
                    "Parsed mmcblk device: index={}, partition={}",
                    device_index, partition
                );
                return Some((DeviceType::SdMmc, device_index, partition));
            }
        }
    }

    // Can add parsing for other device types here...

    None
}

pub fn parse_device_uuid(_uuid: &str) -> Option<(DeviceType, u8, u8)> {
    // Implement UUID parsing logic here
    todo!("")
}

pub fn parse_device_label(label: &str) -> Option<(DeviceType, u8, u8)> {
    // Implement LABEL parsing logic here
    if !label.starts_with("PARTLABEL=") {
        return None;
    }

    todo!("")
}

pub fn parse_device_partuuid(_partuuid: &str) -> Option<(DeviceType, u8, u8)> {
    // Implement PARTUUID parsing logic here
    todo!("")
}

pub fn parse_major_minor(_major: &u32, _minor: &u32) -> Option<(DeviceType, u8, u8)> {
    // Implement major:minor parsing logic here
    todo!("")
}
