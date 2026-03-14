use clap::Parser;

use std::path::{Path, PathBuf};
use std::collections::HashSet;

#[derive(Parser)]
#[command(name = "ShaderPreprocessor", about = "Shader preprocessor for GLSL and WGSL")]
struct Args {
    input: std::path::PathBuf,

    #[arg(short = 'I', long)]
    include: Vec<std::path::PathBuf>,

    #[arg(short, long)]
    output: Option<std::path::PathBuf>,
    
    #[arg(long, help = "Emit #line directives (GLSL only)")]
    line_directives: bool,

    #[arg(short = 'D', long = "define", help = "Define a preprocessor symbol (KEY or KEY=VALUE)")]
    defines: Vec<String>,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let result = preprocess(&args.input, &args.include, args.defines, args.line_directives)?;

    let input_parent_dir = args.input.parent();
    let output_file = match args.output {
        Some(value) =>
            if value.is_absolute() { 
                value
            } else {
                input_parent_dir.unwrap_or(Path::new(".")).join(value)
            },
        None => {
            let stem = args.input.file_stem().unwrap_or_default();
            let ext = args.input.extension().unwrap_or_default();
            let filename = format!("{}_out.{}", stem.to_string_lossy(), ext.to_string_lossy());

            args.input.parent().unwrap_or(Path::new(".")).join(filename)
        }
    };

    std::fs::write(output_file, result)?;
    Ok(())
}

pub fn preprocess(path: &Path, search_paths: &[PathBuf], defines: Vec<String>, line_directives: bool) -> anyhow::Result<String> {
    let mut visited = HashSet::new();
    let mut output = String::new();

    // Emit defines once, at the very top
    for define in &defines {
        output.push_str(&format_define(define));
    }

    output.push_str(&process_file(path, &mut visited, search_paths, line_directives)?);
    Ok(output)
}

fn format_define(define: &str) -> String {
    match define.split_once('=') {
        Some((key, value)) => format!("#define {} {}\n", key, value),
        None => format!("#define {}\n", define),
    }
}

fn process_file(path: &Path, visited: &mut HashSet<PathBuf>, search_paths: &[PathBuf], line_directives: bool) -> anyhow::Result<String> {
    let canonical = path.canonicalize()?;

    if !visited.insert(canonical.clone()) {
        return Ok(String::new());
    }

    let file_id = visited.len() - 1;

    let source = std::fs::read_to_string(&canonical)?;
    let mut output = String::new();

    // Always search the current file's directory first
    let current_dir = canonical.parent().unwrap();
    let effective_paths: Vec<PathBuf> = std::iter::once(current_dir.to_path_buf())
        .chain(search_paths.iter().cloned())
        .collect();

    if line_directives {
        output.push_str(&format!("#line 1 {} // {}\n", file_id, canonical.display()));
    }

    let mut line_number = 1usize;
    for line in source.lines() {
        if let Some(resolved) = parse_include(line, &effective_paths) {
            let included = process_file(&resolved, visited, search_paths, line_directives)?;
            output.push_str(&included);
            if line_directives {
                output.push_str(&format!("#line {} {} // {}\n", line_number + 1, file_id, canonical.display()));
            }
        } else {
            output.push_str(line);
            output.push('\n');
        }
        line_number += 1;
    }

    Ok(output)
}

fn find_include_file(include: &str, search_paths: &[PathBuf]) -> Option<PathBuf> {
    for dir in search_paths {
        let candidate = dir.join(include);
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

fn parse_include(line: &str, search_paths: &[PathBuf]) -> Option<PathBuf> {
    let line = line.trim();
    let line = line.strip_prefix("#include")?.trim();
    let path = line.strip_prefix('"')?.strip_suffix('"')?;
    find_include_file(path, search_paths)
}