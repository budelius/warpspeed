mod data_source;
mod search;
mod search_item;

pub use data_source::DataSource;
pub(crate) use search::{
    normalized_query, row_has_exact_words, row_title, score_row, text_has_exact_words,
};
