fn main() {
  let docs = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("docs");
  cache_arena::cli::main_with_docs(&cache_arena::engines::builtins(), Some(docs));
}
