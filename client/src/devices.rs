use std::fmt::Write;

use cpal::traits::HostTrait;

#[derive(Clone, Debug, Default)]
pub struct DeviceList {
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
}

pub trait AudioDeviceCatalog {
    type Device;

    fn enumerate(&self) -> DeviceList;
    fn input_named(&self, name: Option<&str>) -> Option<Self::Device>;
    fn output_named(&self, name: Option<&str>) -> Option<Self::Device>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct CpalDeviceCatalog;

fn device_name(device: &cpal::Device) -> Option<String> {
    let mut name = String::new();
    write!(&mut name, "{device}").ok()?;
    Some(name)
}

impl AudioDeviceCatalog for CpalDeviceCatalog {
    type Device = cpal::Device;

    fn enumerate(&self) -> DeviceList {
        let host = cpal::default_host();
        let default_input = host
            .default_input_device()
            .and_then(|device| device_name(&device));
        let default_output = host
            .default_output_device()
            .and_then(|device| device_name(&device));
        let mut inputs = host
            .input_devices()
            .map(|devices| devices.filter_map(|device| device_name(&device)).collect())
            .unwrap_or_default();
        let mut outputs = host
            .output_devices()
            .map(|devices| devices.filter_map(|device| device_name(&device)).collect())
            .unwrap_or_default();
        prioritize_default(&mut inputs, default_input.as_deref());
        prioritize_default(&mut outputs, default_output.as_deref());
        DeviceList { inputs, outputs }
    }

    fn input_named(&self, name: Option<&str>) -> Option<Self::Device> {
        let host = cpal::default_host();
        name.and_then(|wanted| {
            host.input_devices()
                .ok()?
                .find(|device| device_name(device).is_some_and(|name| name == wanted))
        })
        .or_else(|| host.default_input_device())
    }

    fn output_named(&self, name: Option<&str>) -> Option<Self::Device> {
        let host = cpal::default_host();
        name.and_then(|wanted| {
            host.output_devices()
                .ok()?
                .find(|device| device_name(device).is_some_and(|name| name == wanted))
        })
        .or_else(|| host.default_output_device())
    }
}

fn prioritize_default(items: &mut Vec<String>, default: Option<&str>) {
    items.sort();
    items.dedup();
    if let Some(default) = default
        && let Some(index) = items.iter().position(|item| item == default)
    {
        let value = items.remove(index);
        items.insert(0, value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn default_is_prioritized_and_names_deduplicated() {
        let mut names = vec!["B".into(), "A".into(), "B".into()];
        prioritize_default(&mut names, Some("B"));
        assert_eq!(names, ["B", "A"]);
    }

    struct FakeBackend;

    impl AudioDeviceCatalog for FakeBackend {
        type Device = String;

        fn enumerate(&self) -> DeviceList {
            DeviceList {
                inputs: vec!["Fake input".into()],
                outputs: vec!["Fake output".into()],
            }
        }

        fn input_named(&self, name: Option<&str>) -> Option<Self::Device> {
            name.map(str::to_owned)
        }

        fn output_named(&self, name: Option<&str>) -> Option<Self::Device> {
            name.map(str::to_owned)
        }
    }

    #[test]
    fn catalog_boundary_supports_non_cpal_device_handles() {
        let backend = FakeBackend;
        assert_eq!(backend.enumerate().inputs, ["Fake input"]);
        assert_eq!(backend.input_named(Some("Input")), Some("Input".into()));
        assert_eq!(backend.output_named(Some("Output")), Some("Output".into()));
    }
}
