//! One driver as the PC's registry has it. Plain data, so that everything which reasons about a
//! list of drivers does so without a registry underneath it.

/// One entry under `HKLM\SOFTWARE\ASIO`, with the DLL its class id points at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// The registry key's own name, which is what a DAW shows in its driver list.
    pub key: String,
    /// The entry's `Description` value, when it has one.
    pub description: Option<String>,
    pub clsid: String,
    /// `InprocServer32`'s default value, or why it could not be read.
    pub dll: Result<String, String>,
}
