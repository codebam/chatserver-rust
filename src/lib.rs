pub mod hashing {
    use sha2::{Sha256, Digest};

    pub fn get_hash(input: String) -> String {
        let mut hasher = Sha256::new();
        hasher.input(input.into_bytes());
        return format!("{:x}", hasher.result());
    }
}
