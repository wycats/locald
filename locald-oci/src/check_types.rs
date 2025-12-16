
#[cfg(test)]
mod tests {
    use oci_spec::image::ImageConfiguration;

    #[test]
    fn test_types_exist() {
        let _config = ImageConfiguration::default();
    }
}
