use std::collections::HashMap;
use std::io;

use crate::hw5800;

#[derive(Debug, Clone)]
pub enum DeviceType {
    Door,
    Motion,
    Unknown,
}

fn io_errstr(s: &str) -> io::Error {
    io::Error::new(io::ErrorKind::Other, s)
}

impl std::str::FromStr for DeviceType {
    type Err = io::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.to_lowercase() == "door" {
            Ok(DeviceType::Door)
        } else if s.to_lowercase() == "motion" {
            Ok(DeviceType::Motion)
        } else {
            Err(io_errstr("Unknown DeviceType"))
        }
    }
}

pub struct DeviceStore(HashMap<u32, DeviceType>);

fn yes_no(b: u8) -> &'static str {
    if b == 0 {
        "n"
    } else {
        "y"
    }
}

impl DeviceStore {
    pub fn new() -> Self {
        DeviceStore(HashMap::new())
    }

    pub fn load<R: io::BufRead>(r: R) -> io::Result<Self> {
        let mut map: HashMap<u32, DeviceType> = HashMap::new();
        for lw in r.lines() {
            let l = lw?;
            let mut elmts = l.split_whitespace();
            let id: u32 = u32::from_str_radix(
                elmts.next().expect("Bad DeviceStore data"),
                16,
            )
            .expect("Bad DeviceStrore id");
            let ty: DeviceType =
                elmts.next().expect("Bad DeviceStore data").parse()?;
            println!("Found device: {:X} {:?}", id, ty);
            map.insert(id, ty);
        }
        Ok(DeviceStore(map))
    }

    pub fn as_json(&self, status: &hw5800::HW5800Status) -> String {
        match self.0.get(&status.id()).unwrap_or(&DeviceType::Unknown) {
            DeviceType::Door => format!(
                r#"{{"open":"{}","tog":"{}","b":"{:02X}"}}"#,
                yes_no(status.bits() & 0b00100000),
                yes_no(status.bits() & 0b01000000),
                // maybe 0x00000100 is the poll bit (i.e. it means "no change in state")
                status.bits()
            ),
            DeviceType::Motion => format!(
                r#"{{"motion":"{}","tog":"{}","b":"{:02X}"}}"#,
                yes_no(status.bits() & 0b10000000),
                yes_no(status.bits() & 0b01000000),
                status.bits()
            ),
            DeviceType::Unknown => {
                format!(r#"{{"b":"{:02X}"}}"#, status.bits())
            }
        }
    }
}
