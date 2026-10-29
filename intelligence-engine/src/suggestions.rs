use crate::models::Suggestion;
use futures::stream;

// Operation that we want to conduct periodically
// -> query DB -> run model -> emit [Action] -> update session_state
// when I create a session, I want to spawn a thread that has a handle, and th

pub(crate) struct SuggestionReader {}

impl stream::Stream for SuggestionReader {
    type Item = Vec<Suggestion>;

    fn poll_next(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        todo!()
    }
}
