fn count_done_sentinels(text: &str) -> usize {
    text.lines().filter(|line| *line == "data: [DONE]").count()
}
