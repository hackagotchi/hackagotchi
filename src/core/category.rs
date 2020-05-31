use rusoto_dynamodb::AttributeValue;
use serde::{Deserialize, Serialize};
use std::convert::TryInto;
use std::fmt;

// A searchable category in the market. May or may not
// correspond 1:1 to an Archetype.
#[derive(Copy, Clone, Debug, PartialEq, Deserialize, Serialize)]
pub enum Category {
    Profile = 0,
    Gotchi = 1,
    Misc = 2,
    Land = 3,
    Sale = 9,
}

impl fmt::Display for Category {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", format!("{:?}", self).to_lowercase())
    }
}

impl Category {
    /*
    fn iter() -> impl ExactSizeIterator<Item = Category> {
        use Category::*;
        [Profile, Gotchi, Misc, Sale].iter().cloned()
    }*/

    pub fn from_av(av: &AttributeValue) -> Result<Self, CategoryError> {
        av.n.as_ref()
            .ok_or(CategoryError::InvalidAttributeValue)?
            .parse::<u8>()?
            .try_into()
    }

    pub fn into_av(self) -> AttributeValue {
        AttributeValue {
            n: Some((self as u8).to_string()),
            ..Default::default()
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum CategoryError {
    UnknownCategory,
    InvalidAttributeValue,
    InvalidNumber(std::num::ParseIntError),
}
impl fmt::Display for CategoryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use CategoryError::*;

        match self {
            UnknownCategory => write!(f, "Unknown Category!"),
            InvalidAttributeValue => write!(f, "Category AttributeValue wasn't a number!"),
            InvalidNumber(e) => {
                write!(f, "Couldn't parse number in Category AttributeValue: {}", e)
            }
        }
    }
}

impl From<std::num::ParseIntError> for CategoryError {
    fn from(o: std::num::ParseIntError) -> Self {
        CategoryError::InvalidNumber(o)
    }
}

impl std::convert::TryFrom<u8> for Category {
    type Error = CategoryError;

    fn try_from(o: u8) -> Result<Self, Self::Error> {
        use Category::*;

        Ok(match o {
            0 => Profile,
            1 => Gotchi,
            2 => Misc,
            3 => Land,
            9 => Sale,
            _ => return Err(CategoryError::UnknownCategory),
        })
    }
}
