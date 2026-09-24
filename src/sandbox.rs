use std::path::Path;

use landlock::{
    path_beneath_rules, Access, AccessFs, CompatLevel, Compatible, RestrictionStatus, Ruleset,
    RulesetAttr, RulesetCreatedAttr, RulesetStatus, ABI,
};

pub fn restrict(fixture: &Path) -> Result<RestrictionStatus, String> {
    let abi = ABI::V3;
    let status = Ruleset::default()
        .set_compatibility(CompatLevel::HardRequirement)
        .handle_access(AccessFs::from_all(abi))
        .map_err(|e| e.to_string())?
        .create()
        .map_err(|e| e.to_string())?
        .add_rules(path_beneath_rules([fixture], AccessFs::from_all(abi)))
        .map_err(|e| e.to_string())?
        .add_rules(path_beneath_rules(
            ["/usr", "/lib", "/lib64", "/bin", "/proc", "/etc", "/dev"],
            AccessFs::from_read(abi),
        ))
        .map_err(|e| e.to_string())?
        .restrict_self()
        .map_err(|e| e.to_string())?;
    if status.ruleset == RulesetStatus::NotEnforced {
        return Err(format!("landlock not enforced: {status:?}"));
    }
    Ok(status)
}
