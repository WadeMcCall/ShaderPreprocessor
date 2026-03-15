use clap::Parser;

use std::path::{Path, PathBuf};
use std::collections::{HashSet, HashMap};

enum Directive {
    Include(PathBuf),
    IfDef(String),
    IfNDef(String),
    Else,
    EndIf,
    None,
    Version(String),
}

fn parse_directive(line: &str, search_paths: &[PathBuf]) -> Directive {
    let trimmed = line.trim();
    if let Some(path) = parse_include(trimmed, search_paths) {
        Directive::Include(path)
    } else if let Some(key) = parse_ifdef(trimmed) {
        Directive::IfDef(key.to_string())
    } else if let Some(key) = parse_ifndef(trimmed) {
        Directive::IfNDef(key.to_string())
    } else if trimmed == "#else" {
        Directive::Else
    } else if trimmed == "#endif" {
        Directive::EndIf
    } else if let Some(rest) = trimmed.strip_prefix("#version") {
        Directive::Version(rest.trim().to_string())
    } else {
        Directive::None
    } 
}

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

    #[arg(short = 'd', long = "internal-define", help = "Define a preprocessor symbol (KEY or KEY=VALUE) for use in preprocessing only, not emitted in output)")]
    internal_defines: Vec<String>,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let result = preprocess(&args.input, &args.include, args.defines, args.internal_defines, args.line_directives)?;

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

pub fn preprocess(path: &Path, search_paths: &[PathBuf], defines: Vec<String>, internal_defines: Vec<String>, line_directives: bool) -> anyhow::Result<String> {
    let mut visited = HashSet::new();
    let mut output = String::new();

    // Peek at the first line to check for #version
    let source = std::fs::read_to_string(path)?;
    let first_line = source.lines().next().unwrap_or("");
    if let Some(rest) = first_line.trim().strip_prefix("#version") {
        output.push_str(&format!("#version {}\n", rest.trim()));
    }

    if line_directives {
        output.push_str(&format!("#line 1 0 // {}\n", path.display()));
    }

    // Emit defines
    for define in &defines {
        output.push_str(&format_define(define));
    }

    let define_map = create_define_map(defines, internal_defines);
    output.push_str(&process_file(path, &mut visited, search_paths, line_directives, &define_map)?);
    Ok(output)
}

fn format_define(define: &str) -> String {
    match define.split_once('=') {
        Some((key, value)) => format!("#define {} {}\n", key, value),
        None => format!("#define {}\n", define),
    }
}

fn create_define_map(defines: Vec<String>, internal_defines: Vec<String>) -> HashMap<String, Option<String>> {
    let mut define_map = HashMap::new();
    for define in defines.into_iter().chain(internal_defines.into_iter()) {
        let (key, value) = match define.split_once('=') {
            Some((k, v)) => (k.to_string(), Some(v.to_string())),
            None => (define, None),
        };
        define_map.insert(key, value);
    }
    define_map
}

fn process_file(path: &Path, visited: &mut HashSet<PathBuf>, search_paths: &[PathBuf], line_directives: bool, define_map: &HashMap<String, Option<String>>) -> anyhow::Result<String> {
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

    let mut emit_stack: Vec<bool> = vec![true];
    let mut line_number = 1usize;
    let mut emitting = true;
    for line in source.lines() {
        match parse_directive(line, &effective_paths) {
            Directive::Include(resolved) => { 
                if !emitting { 
                    line_number += 1;
                    continue;
                }
                let included = process_file(&resolved, visited, search_paths, line_directives, define_map)?;
                output.push_str(&included);
                if line_directives {
                        output.push_str(&format!("#line {} {} // {}\n", line_number + 1, file_id, canonical.display()));
                }
            }
            Directive::IfDef(key) => { 
                emit_stack.push(define_map.contains_key(&key));
                emitting = emit_stack.iter().all(|&b| b);    
            }
            Directive::IfNDef(key) => { 
                emit_stack.push(!define_map.contains_key(&key));
                emitting = emit_stack.iter().all(|&b| b);    
            }
            Directive::Else => { 
                if emit_stack.len() == 1 {
                    anyhow::bail!("unexpected #else without #ifdef in {}", canonical.display());
                }
                if let Some(top) = emit_stack.last_mut() {
                    *top = !*top;
                }
                emitting = emit_stack.iter().all(|&b| b);    
            }
            Directive::EndIf => { 
                if emit_stack.len() == 1 {
                    anyhow::bail!("unexpected #endif without #ifdef in {}", canonical.display());
                }
                emit_stack.pop();
                emitting = emit_stack.iter().all(|&b| b);
            }
            Directive::Version(_v) => {
                if file_id != 0 {
                    anyhow::bail!("#version directive must be in the main file ({}), found in {}", visited.iter().next().unwrap().display(), canonical.display());
                }
            }
            Directive::None => { 
                if !emitting { 
                    line_number += 1;
                    continue;
                }
                output.push_str(line);
                output.push('\n');
            }
        }
        line_number += 1;
    }

    if emit_stack.len() > 1 {
        anyhow::bail!("unclosed #ifdef in {}", canonical.display());
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

fn parse_ifdef(line: &str) -> Option<&str> {
    let line = line.trim();
    line.strip_prefix("#ifdef").map(|s| s.trim())
}

fn parse_ifndef(line: &str) -> Option<&str> {
    let line = line.trim();
    line.strip_prefix("#ifndef").map(|s| s.trim())
}