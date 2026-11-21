#[allow(dead_code)]
#[path = "../src/parquet_output.rs"]
mod parquet_output;
use parquet_output::{CampaignWriter, UserWriter};

fn main() -> std::io::Result<()> {
    let root = std::path::PathBuf::from(std::env::args_os().nth(1).expect("output folder"));
    std::fs::create_dir_all(&root)?;
    let mut writer = UserWriter::new(root.join("boundary"))?;
    for i in 0..131_077i64 {
        let id = format!("u{i}\0\r\tΩ");
        let category = (i >= 131_072 && i % 2 == 0).then_some("é\r".as_bytes());
        writer.push(id.as_bytes(), (i % 14) as i32, i + 1, i * 2, i * 100, category)?;
    }
    assert_eq!(writer.finish()?, 131_077);
    UserWriter::new(root.join("empty_user"))?.finish()?;
    CampaignWriter::new(root.join("empty_campaign"))?.finish()?;
    Ok(())
}
