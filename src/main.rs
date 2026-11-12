mod codegen;
mod lex;
mod opt;
mod parse;
mod preprocess;
mod types;

use std::process::{exit, Command};

fn die(msg: &str) -> ! {
    eprintln!("rcc: {}", msg);
    exit(1)
}

fn run(cmd: &mut Command) {
    match cmd.status() {
        Ok(s) if s.success() => {}
        Ok(s) => exit(s.code().unwrap_or(1)),
        Err(e) => die(&format!("cannot run {:?}: {}", cmd.get_program(), e)),
    }
}

#[derive(Default)]
struct Opts {
    include_paths: Vec<String>,
    after_paths: Vec<String>,     // -idirafter
    defines: Vec<(bool, String)>, // (is -D, argument)
    includes: Vec<String>,        // -include
    pic: bool,                    // -fPIC or -shared
    debug: bool,                  // -g
}

/// Returns the tokens of the builtin prelude and of the translation unit.
fn preprocess(path: &str, opts: &Opts) -> (Vec<lex::Token>, Vec<lex::Token>) {
    let mut pp = preprocess::Preprocessor::new(opts.include_paths.clone(), opts.after_paths.clone());
    for (def, arg) in &opts.defines {
        match (def, arg.split_once('=')) {
            (false, _) => pp.undef(arg),
            (true, Some((name, body))) => pp.define(name, body),
            (true, None) => pp.define(arg, "1"),
        }
    }
    let mut toks = vec![];
    for f in opts.includes.iter().map(|s| s.as_str()).chain([path]) {
        toks.pop(); // the end-of-file token of the previous file
        toks.extend(pp.run(f));
    }
    (pp.prelude, toks)
}

fn compile(path: &str, opts: &Opts) -> String {
    let (mut toks, unit) = preprocess(path, opts);
    toks.extend(unit);
    let toks = lex::convert(toks);
    let (globals, top_asm) = parse::parse(toks);
    codegen::codegen(&globals, &top_asm, opts.pic, opts.debug)
}

fn with_ext(path: &str, ext: &str) -> String {
    let name = std::path::Path::new(path).file_stem().unwrap().to_string_lossy().into_owned();
    format!("{}.{}", name, ext)
}

/// Writes a Makefile dependency rule for the files read since `first_file`.
fn write_deps(out: &str, target: &str, first_file: u32, system: bool, phony: bool) {
    let mut deps: Vec<String> = vec![];
    for name in (first_file..lex::file_count()).map(lex::file_name) {
        if !name.starts_with('<') && (system || !name.starts_with("/usr/")) && !deps.contains(&name) {
            deps.push(name);
        }
    }
    let mut text = format!("{}: {}\n", target, deps.join(" \\\n  "));
    if phony {
        for d in deps.iter().skip(1) {
            text += &format!("{}:\n", d);
        }
    }
    if out == "-" {
        print!("{}", text);
    } else {
        std::fs::write(out, text).unwrap_or_else(|e| die(&format!("{}: {}", out, e)));
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (mut out, mut opt_c, mut opt_s, mut opt_e) = (None::<String>, false, false, false);
    // Dependency output: -MD/-MMD (alongside compilation) or -M/-MM (instead of it).
    let (mut dep_on, mut dep_only, mut dep_system, mut dep_phony) = (false, false, true, false);
    let (mut dep_file, mut dep_target) = (None::<String>, None::<String>);
    let mut link: Vec<String> = vec![]; // objects, libraries and linker options, in order
    let mut inputs: Vec<(usize, String, bool)> = vec![]; // (position in `link`, source file, is C)
    let mut force_c = false; // after `-x c`
    let mut opts = Opts::default();
    let mut temps: Vec<String> = vec![];
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        // Options taking a value, either joined (-Idir) or as the next argument (-I dir).
        const VALUED: [&str; 16] = ["-o", "-I", "-D", "-U", "-include", "-isystem", "-iquote", "-idirafter", "-MF", "-MT", "-MQ", "-x", "-L", "-l", "-Xlinker", "-z"];
        if let Some(opt) = VALUED.iter().find(|o| a.starts_with(**o) && (a.len() == o.len() || o.len() == 2)) {
            let v = if a.len() > opt.len() {
                a[opt.len()..].to_string()
            } else {
                i += 1;
                args.get(i).cloned().unwrap_or_else(|| die(&format!("missing argument to {}", a)))
            };
            match *opt {
                "-o" => out = Some(v),
                "-I" | "-isystem" | "-iquote" => opts.include_paths.push(v),
                "-idirafter" => opts.after_paths.push(v),
                "-D" => opts.defines.push((true, v)),
                "-U" => opts.defines.push((false, v)),
                "-include" => opts.includes.push(v),
                "-MF" => dep_file = Some(v),
                "-MT" | "-MQ" => dep_target = Some(v),
                "-x" => force_c = v == "c", // other languages: decided by the file name
                "-L" | "-l" => link.push(format!("{}{}", opt, v)),
                _ => link.extend([opt.to_string(), v]),
            }
            i += 1;
            continue;
        }
        match a {
            "-c" => opt_c = true,
            "-S" => opt_s = true,
            "-E" => opt_e = true,
            "-MD" => dep_on = true,
            "-MMD" => (dep_on, dep_system) = (true, false),
            "-M" => (dep_on, dep_only) = (true, true),
            "-MM" => (dep_on, dep_only, dep_system) = (true, true, false),
            "-MP" => dep_phony = true,
            "--version" | "-v" | "-V" => {
                println!("rcc {} (C11 compiler for x86-64 Linux)", env!("CARGO_PKG_VERSION"));
                if args.len() == 1 {
                    return;
                }
            }
            "-dumpversion" => return println!("{}", env!("CARGO_PKG_VERSION")),
            "-dumpmachine" => return println!("x86_64-linux-gnu"),
            "-" => {
                // Source on standard input.
                let path = format!("{}/rcc-{}-stdin.c", std::env::temp_dir().display(), std::process::id());
                let mut src = vec![];
                std::io::Read::read_to_end(&mut std::io::stdin(), &mut src).unwrap_or_else(|e| die(&format!("stdin: {}", e)));
                std::fs::write(&path, src).unwrap_or_else(|e| die(&format!("{}: {}", path, e)));
                temps.push(path.clone());
                inputs.push((link.len(), path.clone(), true));
                link.push(path);
            }
            "-fPIC" | "-fpic" => opts.pic = true,
            "-g0" => opts.debug = false,
            _ if a.starts_with("-g") => opts.debug = true,
            "-shared" => {
                opts.pic = true;
                link.push(a.to_string());
            }
            "-s" | "-static" | "-pthread" | "-rdynamic" | "-pie" | "-no-pie" | "-nostdlib" | "-nostartfiles" | "-nodefaultlibs"
            | "-static-libgcc" | "-static-pie" => link.push(a.to_string()),
            _ if a.starts_with("-Wl,") => link.push(a.to_string()),
            _ if a.starts_with('-') => {} // -O2, -g, -W..., -std=..., -f..., -m...: ignored
            _ => {
                if force_c || a.ends_with(".c") || a.ends_with(".s") || a.ends_with(".S") {
                    inputs.push((link.len(), a.to_string(), force_c || a.ends_with(".c")));
                }
                link.push(a.to_string());
            }
        }
        i += 1;
    }
    let no_link = opt_c || opt_s || opt_e || dep_only;
    if inputs.is_empty() && (no_link || link.iter().all(|l| l.starts_with('-'))) {
        die("no input files");
    }
    if out.is_some() && no_link && inputs.len() > 1 {
        die("cannot specify -o with -c, -S or -E with multiple files");
    }

    for (n, (slot, input, is_c)) in inputs.iter().enumerate() {
        let tmp = |ext: &str| format!("{}/rcc-{}-{}.{}", std::env::temp_dir().display(), std::process::id(), n, ext);
        let obj = if opt_c { out.clone().unwrap_or_else(|| with_ext(input, "o")) } else { tmp("o") };
        if !is_c {
            // Assembly sources go to the system assembler (through cc, for .S).
            if opt_c {
                run(Command::new("cc").args(["-c", "-o", &obj, input]));
            }
            continue;
        }
        let first_file = lex::file_count();
        if opt_e || dep_only {
            let toks = preprocess(input, &opts).1;
            if dep_only {
                let target = dep_target.clone().unwrap_or_else(|| with_ext(input, "o"));
                write_deps(dep_file.as_deref().or(out.as_deref()).unwrap_or("-"), &target, first_file, dep_system, dep_phony);
                continue;
            }
            let text = preprocess::to_text(&toks);
            match &out {
                Some(o) => std::fs::write(o, text + "\n").unwrap_or_else(|e| die(&format!("{}: {}", o, e))),
                None => println!("{}", text),
            }
            continue;
        }
        let asm = compile(input, &opts);
        let asm_path = if opt_s { out.clone().unwrap_or_else(|| with_ext(input, "s")) } else { tmp("s") };
        std::fs::write(&asm_path, asm).unwrap_or_else(|e| die(&format!("{}: {}", asm_path, e)));
        if dep_on {
            let main_out = if opt_s { &asm_path } else { &obj };
            let file = dep_file.clone().unwrap_or_else(|| std::path::Path::new(main_out).with_extension("d").to_string_lossy().into_owned());
            write_deps(&file, dep_target.as_deref().unwrap_or(main_out), first_file, dep_system, dep_phony);
        }
        if opt_s {
            continue;
        }
        temps.push(asm_path.clone());
        run(Command::new("as").args(["-o", &obj, &asm_path]));
        if !opt_c {
            temps.push(obj.clone());
            link[*slot] = obj;
        }
    }
    if !no_link {
        let exe = out.unwrap_or_else(|| "a.out".to_string());
        // The system C compiler is used only as the linker driver (crt files, libc).
        run(Command::new("cc").args(["-o", &exe]).args(&link).arg("-lm"));
    }
    for t in temps {
        let _ = std::fs::remove_file(t);
    }
}
