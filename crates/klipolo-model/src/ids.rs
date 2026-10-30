/// Entity identifier. Model-created ids are UUID v4 strings.
pub type Id = String;

pub fn new_id() -> Id {
    uuid::Uuid::new_v4().to_string()
}
