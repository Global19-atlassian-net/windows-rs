use super::*;
use crate::TypeReader;

#[derive(Copy, Clone)]
pub struct TypeRef {
    pub reader: &'static TypeReader,
    pub row: Row,
}

impl TypeRef {
    pub fn scope(&self) -> ResolutionScope {
        self.reader.decode(self.row, 0)
    }

    pub fn name(&self) -> (&'static str, &'static str) {
        (self.reader.str(self.row, 2), self.reader.str(self.row, 1))
    }

    // TODO: still need a way to get the list of nested types for a given struct so they can be defined.
    // Provide that in the TypeReader and then the following function can use that once it has the enclosing type def.

    // TODO: push this method into the TypeReader?
    pub fn resolve(&self) -> TypeDef {
        if let ResolutionScope::TypeRef(scope) = self.scope() {
            let enclosing_type = self.reader.expect_type_def(scope.name());
            let row = self.reader.nested_types.get(&enclosing_type.row).expect("TypeRef::resolve")
                .iter()
                .find(|nested_type| {
                    self.reader.str(**nested_type, 1) == self.reader.str(self.row, 1)
                })
                .expect(&format!("Could not find nested type `{}`", self.name().1));

            TypeDef {
                reader: self.reader,
                row: *row,
            }
        } else {
            self.reader.expect_type_def(self.name())
        }
    }
}

impl std::fmt::Debug for TypeRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TypeRef").field("row", &self.row).finish()
    }
}

impl PartialEq for TypeRef {
    fn eq(&self, other: &Self) -> bool {
        self.row == other.row
    }
}

impl Eq for TypeRef {}

impl Ord for TypeRef {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.row.cmp(&other.row)
    }
}

impl PartialOrd for TypeRef {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
