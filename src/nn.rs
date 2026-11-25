//! Neural-network diagrams: a Rust port of PlotNeuralNet's pycore/tikzeng.py
//! (https://github.com/HarisIqbal88/PlotNeuralNet, MIT, see vendor/plotneuralnet/LICENSE),
//! driven by a one-layer-per-line text format instead of a Python script.
//!
//! ```text
//! # kind      name    key=value ...
//! input       img     file=../figures/cat.png
//! conv        conv1   s=512 n=64 w=2 h=64 d=64 caption=Conv1
//! pool        pool1
//! conv        conv2   s=128 n=64 w=2 h=32 d=32
//! convrelu    ccr1    n=64,64 w=2,2
//! softmax     soft1   s=10 caption=SOFT gap=3
//! sum         sum1    gap=1.5
//! skip        conv1 conv2
//! ```
//! Each layer sits `gap` units east of the previous one (or of `at=<name>`); an arrow joins
//! them when gap > 0. pool/unpool default to gap 0 (attached), everything else to gap 1.

/// The TikZ shapes the generated file loads (`\subimport{layers/}{init}`).
pub const LAYERS: [(&str, &str); 4] = [
    ("init.tex", include_str!("../vendor/plotneuralnet/init.tex")),
    ("Box.sty", include_str!("../vendor/plotneuralnet/Box.sty")),
    ("Ball.sty", include_str!("../vendor/plotneuralnet/Ball.sty")),
    ("RightBandedBox.sty", include_str!("../vendor/plotneuralnet/RightBandedBox.sty")),
];

pub const EXAMPLE: &str = "# One layer per line: kind name key=value ...  (# starts a comment)
conv     conv1  s=512 n=64 w=2 h=64 d=64 caption=Conv1
pool     pool1
conv     conv2  s=128 n=64 w=2 h=32 d=32 caption=Conv2
pool     pool2  h=28 d=28
softmax  soft1  s=10 caption=Softmax gap=3
sum      sum1   gap=1.5
";

// ---- tikzeng.py, function for function ----

pub fn to_head(layers: &str) -> String {
    format!("\\documentclass[border=8pt, multi, tikz]{{standalone}}\n\\usepackage{{import}}\n\\subimport{{{layers}}}{{init}}\n\\usetikzlibrary{{positioning}}\n\\usetikzlibrary{{3d}} %for including external image\n")
}

pub const TO_COR: &str = r"
\def\ConvColor{rgb:yellow,5;red,2.5;white,5}
\def\ConvReluColor{rgb:yellow,5;red,5;white,5}
\def\PoolColor{rgb:red,1;black,0.3}
\def\UnpoolColor{rgb:blue,2;green,1;black,0.3}
\def\FcColor{rgb:blue,5;red,2.5;white,5}
\def\FcReluColor{rgb:blue,5;red,5;white,4}
\def\SoftmaxColor{rgb:magenta,5;black,7}
\def\SumColor{rgb:blue,5;green,15}
";

pub const TO_BEGIN: &str = r"
\newcommand{\copymidarrow}{\tikz \draw[-Stealth,line width=0.8mm,draw={rgb:blue,4;red,1;green,1;black,3}] (-0.3,0) -- ++(0.3,0);}

\begin{document}
\begin{tikzpicture}
\tikzstyle{connection}=[ultra thick,every node/.style={sloped,allow upside down},draw=\edgecolor,opacity=0.7]
\tikzstyle{copyconnection}=[ultra thick,every node/.style={sloped,allow upside down},draw={rgb:blue,4;red,1;green,1;black,3},opacity=0.7]
";

pub const TO_END: &str = "\n\\end{tikzpicture}\n\\end{document}\n";

pub fn to_input(file: &str, to: &str, width: f64, height: f64, name: &str) -> String {
    format!("\n\\node[canvas is zy plane at x=0] ({name}) at {to} {{\\includegraphics[width={width}cm,height={height}cm]{{{file}}}}};\n")
}

/// `\pic` of one of the layer shapes; `keys` are its `key=value` options.
fn pic(shape: &str, offset: &str, to: &str, keys: &[String]) -> String {
    format!("\n\\pic[shift={{{offset}}}] at {to}\n    {{{shape}={{\n        {}\n        }}\n    }};\n", keys.join(",\n        "))
}

/// The settings every layer function takes (Python's keyword arguments).
#[derive(Clone)]
pub struct L {
    pub name: String,
    pub s: String,      // s_filer: size label (zlabel)
    pub n: Vec<String>, // n_filer: filter-count label(s) (xlabel)
    pub offset: String,
    pub to: String,
    pub w: Vec<f64>,
    pub h: f64,
    pub d: f64,
    pub opacity: f64,
    pub caption: String,
    pub radius: f64,
}

fn base(l: &L) -> Vec<String> {
    vec![format!("name={}", l.name), format!("caption={{{}}}", l.caption)]
}
fn size(l: &L) -> Vec<String> {
    vec![format!("height={}", l.h), format!("width={}", l.w[0]), format!("depth={}", l.d)]
}

pub fn to_conv(l: &L) -> String {
    let k = [base(l), vec![format!("xlabel={{{{{}, }}}}", l.n[0]), format!("zlabel={{{}}}", l.s), "fill=\\ConvColor".into()], size(l)].concat();
    pic("Box", &l.offset, &l.to, &k)
}

pub fn to_conv_conv_relu(l: &L) -> String {
    let (n2, w2) = (l.n.get(1).unwrap_or(&l.n[0]), l.w.get(1).unwrap_or(&l.w[0]));
    let k = [base(l), vec![
        format!("xlabel={{{{ {}, {n2} }}}}", l.n[0]), format!("zlabel={{{}}}", l.s),
        "fill=\\ConvColor".into(), "bandfill=\\ConvReluColor".into(),
        format!("height={}", l.h), format!("width={{ {} , {w2} }}", l.w[0]), format!("depth={}", l.d),
    ]].concat();
    pic("RightBandedBox", &l.offset, &l.to, &k)
}

fn pool_like(l: &L, fill: &str) -> String {
    let k = [base(l), vec![format!("fill={fill}"), format!("opacity={}", l.opacity)], size(l)].concat();
    pic("Box", &l.offset, &l.to, &k)
}
pub fn to_pool(l: &L) -> String {
    pool_like(l, "\\PoolColor")
}
pub fn to_unpool(l: &L) -> String {
    pool_like(l, "\\UnpoolColor")
}

pub fn to_conv_res(l: &L) -> String {
    let k = [base(l), vec![
        format!("xlabel={{{{ {}, }}}}", l.n[0]), format!("zlabel={{{}}}", l.s),
        "fill={rgb:white,1;black,3}".into(), "bandfill={rgb:white,1;black,2}".into(), format!("opacity={}", l.opacity),
    ], size(l)].concat();
    pic("RightBandedBox", &l.offset, &l.to, &k)
}

pub fn to_conv_softmax(l: &L) -> String {
    let k = [base(l), vec![format!("zlabel={{{}}}", l.s), "fill=\\SoftmaxColor".into()], size(l)].concat();
    pic("Box", &l.offset, &l.to, &k)
}

pub fn to_softmax(l: &L) -> String {
    let k = [base(l), vec![
        "xlabel={{\" \",\"dummy\"}}".into(), format!("zlabel={{{}}}", l.s), "fill=\\SoftmaxColor".into(), format!("opacity={}", l.opacity),
    ], size(l)].concat();
    pic("Box", &l.offset, &l.to, &k)
}

pub fn to_sum(l: &L) -> String {
    let k = vec![format!("name={}", l.name), "fill=\\SumColor".into(), format!("opacity={}", l.opacity), format!("radius={}", l.radius), "logo=$+$".into()];
    pic("Ball", &l.offset, &l.to, &k)
}

pub fn to_connection(of: &str, to: &str) -> String {
    format!("\n\\draw [connection]  ({of}-east)    -- node {{\\midarrow}} ({to}-west);\n")
}

pub fn to_skip(of: &str, to: &str, pos: f64) -> String {
    format!("
\\path ({of}-southeast) -- ({of}-northeast) coordinate[pos={pos}] ({of}-top) ;
\\path ({to}-south)  -- ({to}-north)  coordinate[pos={pos}] ({to}-top) ;
\\draw [copyconnection]  ({of}-northeast)
-- node {{\\copymidarrow}}({of}-top)
-- node {{\\copymidarrow}}({to}-top)
-- node {{\\copymidarrow}} ({to}-north);
")
}

// ---- the text format ----

/// Python's per-function defaults: (s, n, w, h, d, opacity, gap).
fn defaults(kind: &str) -> Option<(&'static str, &'static str, &'static str, f64, f64, f64, f64)> {
    Some(match kind {
        "conv" => ("256", "64", "1", 40.0, 40.0, 1.0, 1.0),
        "convrelu" => ("256", "64,64", "2,2", 40.0, 40.0, 1.0, 1.0),
        "pool" | "unpool" => ("", "", "1", 32.0, 32.0, 0.5, 0.0),
        "convres" => ("256", "64", "6", 40.0, 40.0, 0.2, 1.0),
        "convsoftmax" => ("40", "", "1", 40.0, 40.0, 1.0, 1.0),
        "softmax" => ("10", "", "1.5", 3.0, 25.0, 0.8, 1.0),
        "sum" => ("", "", "1", 0.0, 0.0, 0.6, 1.0),
        _ => return None,
    })
}

/// Split on spaces, keeping `key="two words"` together.
fn tokens(line: &str) -> Vec<String> {
    let (mut out, mut cur, mut quoted) = (vec![], String::new(), false);
    for c in line.chars() {
        match c {
            '"' => quoted = !quoted,
            c if c.is_whitespace() && !quoted => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn number(v: &str) -> Result<f64, String> {
    v.parse::<f64>().ok().filter(|x| x.is_finite() && *x >= 0.0).ok_or_else(|| format!("'{v}' is not a number"))
}

/// Labels and captions are typeset; refuse what would break out of the TikZ options.
fn label(v: &str) -> Result<String, String> {
    let mut depth = 0i32;
    for c in v.chars() {
        depth += match c { '{' => 1, '}' => -1, _ => 0 };
        if depth < 0 {
            return Err(format!("unbalanced braces in '{v}'"));
        }
    }
    if depth != 0 || v.contains('%') {
        return Err(format!("unbalanced braces or % in '{v}'"));
    }
    Ok(v.to_string())
}

fn valid_name(n: &str) -> bool {
    n.chars().next().is_some_and(|c| c.is_ascii_alphabetic()) && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// The whole standalone .tex file for a network written in the text format.
pub fn build(spec: &str, layers_dir: &str) -> Result<String, String> {
    let (mut body, mut names, mut prev): (String, Vec<String>, Option<String>) = (String::new(), vec![], None);
    for (i, raw) in spec.lines().enumerate() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let err = |m: String| format!("Line {}: {m}", i + 1);
        let t = tokens(line);
        let kind = t[0].to_lowercase();
        let known = |n: &str| if names.iter().any(|x| x == n) { Ok(n.to_string()) } else { Err(err(format!("no layer named '{n}' above this line"))) };
        if kind == "skip" || kind == "connect" {
            let (Some(a), Some(b)) = (t.get(1), t.get(2)) else { return Err(err(format!("write: {kind} <from> <to>"))) };
            let (a, b) = (known(a)?, known(b)?);
            let pos = t.iter().skip(3).find_map(|kv| kv.strip_prefix("pos=")).map(number).transpose().map_err(err)?.unwrap_or(1.25);
            body += &if kind == "skip" { to_skip(&a, &b, pos) } else { to_connection(&a, &b) };
            continue;
        }
        let name = t.get(1).ok_or_else(|| err("missing layer name".into()))?.clone();
        if !valid_name(&name) {
            return Err(err(format!("'{name}': names use letters, digits and _ and start with a letter")));
        }
        if names.contains(&name) {
            return Err(err(format!("'{name}' is already used")));
        }
        let mut kv = std::collections::HashMap::new();
        for p in &t[2..] {
            let (k, v) = p.split_once('=').ok_or_else(|| err(format!("'{p}' should be key=value")))?;
            kv.insert(k.to_lowercase(), v.to_string());
        }
        let get = |k: &str| kv.get(k).map(String::as_str);
        if kind == "input" {
            let file = get("file").ok_or_else(|| err("input needs file=<image path>".into()))?;
            let (w, h) = (number(get("w").unwrap_or("8")).map_err(err)?, number(get("h").unwrap_or("8")).map_err(err)?);
            body += &to_input(&label(file).map_err(err)?, "(-3,0,0)", w, h, &name);
            names.push(name);
            continue;
        }
        let (s, n, w, h, d, opacity, gap) = defaults(&kind).ok_or_else(|| err(format!(
            "unknown layer '{kind}'. Use conv, convrelu, pool, unpool, convres, convsoftmax, softmax, sum, input, skip or connect")))?;
        let anchor = match get("at") { Some(a) => Some(known(a)?), None => prev.clone() };
        let gap = match anchor { Some(_) => number(get("gap").unwrap_or(&gap.to_string())).map_err(err)?, None => 0.0 };
        let l = L {
            s: label(get("s").unwrap_or(s)).map_err(err)?,
            n: get("n").unwrap_or(n).split(',').map(|x| label(x.trim())).collect::<Result<_, _>>().map_err(err)?,
            w: get("w").unwrap_or(w).split(',').map(|x| number(x.trim())).collect::<Result<_, _>>().map_err(err)?,
            h: number(get("h").map_or(h.to_string(), String::from).as_str()).map_err(err)?,
            d: number(get("d").map_or(d.to_string(), String::from).as_str()).map_err(err)?,
            opacity: number(get("opacity").map_or(opacity.to_string(), String::from).as_str()).map_err(err)?,
            radius: number(get("radius").unwrap_or("2.5")).map_err(err)?,
            caption: label(get("caption").unwrap_or(" ")).map_err(err)?,
            offset: format!("({gap},0,0)"),
            to: anchor.as_ref().map_or("(0,0,0)".into(), |a| format!("({a}-east)")),
            name: name.clone(),
        };
        body += &match kind.as_str() {
            "conv" => to_conv(&l),
            "convrelu" => to_conv_conv_relu(&l),
            "pool" => to_pool(&l),
            "unpool" => to_unpool(&l),
            "convres" => to_conv_res(&l),
            "convsoftmax" => to_conv_softmax(&l),
            "softmax" => to_softmax(&l),
            _ => to_sum(&l),
        };
        if let (Some(p), true) = (&prev, gap > 0.0) {
            body += &to_connection(p, &name);
        }
        names.push(name.clone());
        prev = Some(name);
    }
    if names.is_empty() {
        return Err("Add at least one layer, for example: conv conv1".into());
    }
    Ok(to_head(layers_dir) + TO_COR + TO_BEGIN + &body + TO_END)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn example_builds() {
        let t = build(EXAMPLE, "layers/").unwrap();
        assert!(t.starts_with("\\documentclass[border=8pt, multi, tikz]{standalone}") && t.contains("\\subimport{layers/}{init}"));
        assert!(t.contains("name=conv1") && t.contains("xlabel={{64, }}") && t.contains("zlabel={512}"));
        assert!(t.contains("\\pic[shift={(0,0,0)}] at (conv1-east)")); // pool attaches to conv1
        assert!(t.contains("\\pic[shift={(3,0,0)}] at (pool2-east)"));
        assert!(t.contains("(pool1-east)    -- node {\\midarrow} (conv2-west)")); // gap 1 -> arrow
        assert!(!t.contains("(conv1-east)    -- node")); // gap 0 -> no arrow
        assert!(t.contains("logo=$+$") && t.trim_end().ends_with("\\end{document}"));
    }

    #[test]
    fn options_and_skips() {
        let t = build("conv a\nconvrelu b n=32,16 w=2,3 caption=\"Two words\"\nskip a b pos=1.5\nconnect a b", "l/").unwrap();
        assert!(t.contains("xlabel={{ 32, 16 }}") && t.contains("width={ 2 , 3 }") && t.contains("caption={Two words}"));
        assert!(t.contains("coordinate[pos=1.5] (a-top)"));
    }

    #[test]
    fn errors_name_the_line() {
        for (spec, msg) in [
            ("conv a\nfoo b", "Line 2: unknown layer"),
            ("conv a\nconv a", "already used"),
            ("conv a h=tall", "not a number"),
            ("conv a\npool b at=zz", "no layer named 'zz'"),
            ("conv 1x", "names use letters"),
            ("conv a caption=}x{", "unbalanced"),
            ("conv a h=inf", "not a number"),
            ("# only a comment", "at least one layer"),
        ] {
            let e = build(spec, "l/").unwrap_err();
            assert!(e.contains(msg), "{spec}: {e}");
        }
    }
}
