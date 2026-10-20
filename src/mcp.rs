pub fn serve<R: std::io::BufRead, W: std::io::Write + Send + 'static>(
    _: R,
    _: W,
    _: &crate::config::Config,
) -> std::io::Result<()> {
    Ok(())
}
