mod codegen;
mod lex;
mod parse;
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

/// shortcut: preprocessing is delegated to the system `cc -E` (with __GNUC__
/// undefined so that glibc headers avoid GNU extensions); replace with a
/// built-in preprocessor to make rcc self-contained.
fn preprocess(path: &str, cpp_args: &[String]) -> Vec<u8> {
    let out = Command::new("cc")
        .args(["-E", "-P", "-std=gnu11", "-U__GNUC__"])
        .args(cpp_args)
        .arg(path)
        .output()
        .unwrap_or_else(|e| die(&format!("cannot run cc -E: {}", e)));
    if !out.status.success() {
        eprint!("{}", String::from_utf8_lossy(&out.stderr));
        exit(1);
    }
    out.stdout
}

fn compile(path: &str, cpp_args: &[String]) -> String {
    let src = preprocess(path, cpp_args);
    let file = lex::add_file(path);
    let toks = lex::convert(lex::tokenize(&src, file));
    let globals = parse::parse(toks);
    codegen::codegen(&globals)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (mut out, mut opt_c, mut opt_s, mut opt_e) = (None::<String>, false, false, false);
    let (mut cpp_args, mut link_args, mut inputs) = (vec![], vec![], vec![]);
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        match a.as_str() {
            "-o" | "-I" | "-D" | "-U" | "-include" => {
                i += 1;
                let Some(v) = args.get(i) else { die(&format!("missing argument to {}", a)) };
                if a == "-o" {
                    out = Some(v.clone());
                } else {
                    cpp_args.extend([a.clone(), v.clone()]);
                }
            }
            "-c" => opt_c = true,
            "-S" => opt_s = true,
            "-E" => opt_e = true,
            _ if a.starts_with("-o") => out = Some(a[2..].to_string()),
            _ if a.starts_with("-I") || a.starts_with("-D") || a.starts_with("-U") => cpp_args.push(a.clone()),
            _ if a.starts_with("-l") || a.starts_with("-L") || a.starts_with("-Wl,") => link_args.push(a.clone()),
            "-static" | "-shared" | "-pthread" | "-rdynamic" => link_args.push(a.clone()),
            _ if a.starts_with('-') && a.len() > 1 => {} // -O2, -g, -W..., -std=..., -f...: ignored
            _ => inputs.push(a.clone()),
        }
        i += 1;
    }
    if inputs.is_empty() {
        die("no input files");
    }
    if out.is_some() && (opt_c || opt_s || opt_e) && inputs.len() > 1 {
        die("cannot specify -o with -c, -S or -E with multiple files");
    }

    let stem = |p: &str, ext: &str| {
        let name = std::path::Path::new(p).file_stem().unwrap().to_string_lossy().into_owned();
        format!("{}.{}", name, ext)
    };
    let mut objs: Vec<String> = vec![];
    let mut temps: Vec<String> = vec![];
    for (n, input) in inputs.iter().enumerate() {
        if !input.ends_with(".c") {
            objs.push(input.clone());
            continue;
        }
        if opt_e {
            let text = preprocess(input, &cpp_args);
            match &out {
                Some(o) => std::fs::write(o, text).unwrap_or_else(|e| die(&format!("{}: {}", o, e))),
                None => print!("{}", String::from_utf8_lossy(&text)),
            }
            continue;
        }
        let asm = compile(input, &cpp_args);
        let tmp = |ext: &str| format!("{}/rcc-{}-{}.{}", std::env::temp_dir().display(), std::process::id(), n, ext);
        let asm_path = if opt_s { out.clone().unwrap_or_else(|| stem(input, "s")) } else { tmp("s") };
        std::fs::write(&asm_path, asm).unwrap_or_else(|e| die(&format!("{}: {}", asm_path, e)));
        if opt_s {
            continue;
        }
        temps.push(asm_path.clone());
        let obj = if opt_c { out.clone().unwrap_or_else(|| stem(input, "o")) } else { tmp("o") };
        run(Command::new("as").args(["-o", &obj, &asm_path]));
        if !opt_c {
            temps.push(obj.clone());
        }
        objs.push(obj);
    }
    if !(opt_c || opt_s || opt_e) {
        let exe = out.unwrap_or_else(|| "a.out".to_string());
        // The system C compiler is used only as the linker driver (crt files, libc).
        run(Command::new("cc").args(["-o", &exe]).args(&objs).args(&link_args));
    }
    for t in temps {
        let _ = std::fs::remove_file(t);
    }
}
