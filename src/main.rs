use dashmap::DashSet;
use rayon::prelude::*;
use serde::Deserialize;
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::Arc,
};
use swc_core::ecma::{
    ast::{AssignTarget, Callee, Expr, MemberProp, SimpleAssignTarget},
    atoms::JsWord,
    parser::EsSyntax,
    utils::ExprExt,
    visit::VisitWith,
};
use swc_core::{
    common::{sync::Lrc, FileName, Globals, Mark, SourceMap, SyntaxContext},
    ecma::{
        ast::MemberExpr,
        parser::parse_file_as_module,
        transforms::base::resolver,
        visit::{Visit, VisitMutWith},
    },
};

#[derive(Deserialize)]
struct PackageJSON {
    dependencies: HashMap<String, String>,
}

#[derive(Debug, Default)]
struct Stats {
    packages: u32,
    files: u32,
    is_esm: u32,
    dynamic_import: u32,
    is_cjs: u32,
    non_static_exports: u32,
    non_static_deps: u32,
    error: u32,
}

impl Stats {
    fn file() -> Stats {
        Stats {
            files: 1,
            ..Default::default()
        }
    }

    fn error() -> Stats {
        Stats {
            files: 1,
            error: 1,
            ..Default::default()
        }
    }

    fn merge(self, other: Stats) -> Stats {
        Stats {
            packages: self.packages + other.packages,
            files: self.files + other.files,
            is_esm: self.is_esm + other.is_esm,
            dynamic_import: self.dynamic_import + other.dynamic_import,
            is_cjs: self.is_cjs + other.is_cjs,
            non_static_exports: self.non_static_exports + other.non_static_exports,
            non_static_deps: self.non_static_deps + other.non_static_deps,
            error: self.error + other.error,
        }
    }
}

fn main() {
    let pkg = std::fs::read_to_string("package.json").unwrap();
    let pkg: PackageJSON = serde_json::from_str(&pkg).unwrap();
    let fs = Arc::new(parcel_resolver::OsFileSystem::default());
    let cache = parcel_resolver::Cache::new(fs);
    let resolver = parcel_resolver::Resolver::parcel(
        std::env::current_dir().unwrap().into(),
        parcel_resolver::CacheCow::Owned(cache),
    );
    let from = resolver.project_root.join("index.js");

    let visited = DashSet::new();
    let mut stats = pkg
        .dependencies
        .par_iter()
        .map(|(name, _)| {
            // These packages have invalid package.jsons (for our purposes) and are skipped.
            if name.starts_with("@types/")
                || name == "@octokit/openapi-types"
                || name == "@graphql-typed-document-node/core"
                || name == "csstype"
                || name == "@tokenizer/token"
            {
                return Stats::default();
            }
            let resolution = resolver
                .resolve(&name, &from, parcel_resolver::SpecifierType::Esm)
                .result;

            if let Ok((parcel_resolver::Resolution::Path(resolved), _)) = resolution {
                // println!("{:?}", resolved);
                analyze(&resolved, &resolver, &visited)
            } else if let Ok((parcel_resolver::Resolution::Builtin(_), _)) = resolution {
                Stats::default()
            } else {
                // println!("Could not resolve {}", name);
                Stats::default()
            }
        })
        .reduce(Stats::default, Stats::merge);

    let packages: HashSet<_> = visited
        .iter()
        .map(|name| {
            // Take the path up to the last node_modules/xxx (to account for multiple versions of packages).
            let components = name.components().collect::<Vec<_>>();
            let index = components
                .iter()
                .rev()
                .position(|c| c.as_os_str() == "node_modules");
            if let Some(index) = index {
                let index = components.len() - index - 1;
                let mut pkg = components[0..index + 2]
                    .iter()
                    .map(|c| c.as_os_str().to_str().unwrap().to_owned())
                    .collect::<Vec<_>>();
                if pkg[pkg.len() - 1].starts_with("@") {
                    pkg.push(
                        components[index + 2]
                            .as_os_str()
                            .to_str()
                            .unwrap()
                            .to_owned(),
                    );
                }
                pkg.join("/")
            } else {
                "".into()
            }
        })
        .collect();

    stats.packages = packages.len() as u32;
    println!("{:?}", stats);
}

fn analyze(
    path: &Path,
    dep_resolver: &parcel_resolver::Resolver,
    visited: &DashSet<PathBuf>,
) -> Stats {
    if !visited.insert(path.to_path_buf()) {
        return Stats::default();
    }

    let ext = path.extension();
    match ext {
        Some(ext) => {
            if ext == "json" || ext == "node" || ext == "css" || ext == "svg" {
                return Stats::default();
            }
        }
        None => return Stats::default(),
    }

    let source_map = Lrc::new(SourceMap::default());
    let code = match std::fs::read_to_string(path) {
        Ok(code) => code,
        Err(err) => {
            println!("ERROR READING {:?}: {:?}", path, err);
            return Stats::error();
        }
    };

    let source_file =
        source_map.new_source_file(Lrc::new(FileName::Real(path.to_owned())), code.into());
    let mut recovered_errors = Vec::new();
    let mut module = match parse_file_as_module(
        &source_file,
        swc_core::ecma::parser::Syntax::Es(EsSyntax {
            import_attributes: true,
            ..Default::default()
        }),
        Default::default(),
        None,
        &mut recovered_errors,
    ) {
        Ok(module) => module,
        Err(err) => {
            println!("COULD NOT PARSE {:?}: {:?}", path, err);
            return Stats::error();
        }
    };

    let (stats, dependencies) = swc_core::common::GLOBALS.set(&Globals::new(), || {
        let unresolved_mark = Mark::fresh(Mark::root());
        let global_mark = Mark::fresh(Mark::root());
        module.visit_mut_with(&mut resolver(unresolved_mark, global_mark, true));

        let mut analyzer = Analyzer {
            unresolved_mark,
            stats: Stats::file(),
            dependencies: Default::default(),
            source_map,
        };

        module.visit_with(&mut analyzer);
        (analyzer.stats, analyzer.dependencies)
    });

    let dep_stats = dependencies
        .par_iter()
        .map(|dep| {
            let resolution = dep_resolver
                .resolve(dep.as_str(), path, parcel_resolver::SpecifierType::Esm)
                .result;

            if let Ok((parcel_resolver::Resolution::Path(resolved), _)) = resolution {
                analyze(&resolved, dep_resolver, visited)
            } else if let Ok((parcel_resolver::Resolution::Builtin(_), _)) = resolution {
                Stats::default()
            } else {
                // println!("Could not resolve {}", dep);
                Stats::default()
            }
        })
        .reduce(Stats::default, Stats::merge);

    stats.merge(dep_stats)
}

struct Analyzer {
    stats: Stats,
    unresolved_mark: Mark,
    dependencies: HashSet<JsWord>,
    #[allow(unused)]
    source_map: Lrc<SourceMap>,
}

impl Visit for Analyzer {
    fn visit_module_decl(&mut self, n: &swc_core::ecma::ast::ModuleDecl) {
        self.stats.is_esm = 1;
        n.visit_children_with(self);
    }

    fn visit_import_decl(&mut self, n: &swc_core::ecma::ast::ImportDecl) {
        self.dependencies.insert(n.src.value.clone());
        n.visit_children_with(self);
    }

    fn visit_export_all(&mut self, n: &swc_core::ecma::ast::ExportAll) {
        self.dependencies.insert(n.src.value.clone());
        n.visit_children_with(self);
    }

    fn visit_named_export(&mut self, n: &swc_core::ecma::ast::NamedExport) {
        if let Some(src) = &n.src {
            self.dependencies.insert(src.value.clone());
        }
        n.visit_children_with(self);
    }

    fn visit_call_expr(&mut self, n: &swc_core::ecma::ast::CallExpr) {
        if let Callee::Import(_) = &n.callee {
            let src = n.args.first().and_then(|arg| {
                arg.expr
                    .as_pure_string(&swc_core::ecma::utils::ExprCtx {
                        unresolved_ctxt: SyntaxContext::empty().apply_mark(self.unresolved_mark),
                        is_unresolved_ref_safe: false,
                    })
                    .into_result()
                    .ok()
            });
            self.stats.dynamic_import = 1;
            if let Some(src) = src {
                self.dependencies.insert(src.into());
            } else {
                self.stats.non_static_deps = 1;
            }
        }

        if let Callee::Expr(expr) = &n.callee {
            if let Expr::Ident(id) = &**expr {
                if id.sym == "require" && id.ctxt.has_mark(self.unresolved_mark) {
                    self.stats.is_cjs = 1;

                    let src = n.args.first().and_then(|arg| {
                        arg.expr
                            .as_pure_string(&swc_core::ecma::utils::ExprCtx {
                                unresolved_ctxt: SyntaxContext::empty()
                                    .apply_mark(self.unresolved_mark),
                                is_unresolved_ref_safe: false,
                            })
                            .into_result()
                            .ok()
                    });

                    if let Some(src) = src {
                        self.dependencies.insert(src.into());
                    } else {
                        self.stats.non_static_deps = 1;
                    }
                }
            }
        }

        n.visit_children_with(self);
    }

    fn visit_assign_expr(&mut self, n: &swc_core::ecma::ast::AssignExpr) {
        match &n.left {
            AssignTarget::Simple(SimpleAssignTarget::Member(member)) => {
                if self.is_module_exports(member) {
                    self.stats.is_cjs = 1;
                    n.right.visit_with(self);
                    return;
                }
            }
            _ => {}
        }

        n.visit_children_with(self);
    }

    fn visit_member_expr(&mut self, member: &swc_core::ecma::ast::MemberExpr) {
        if self.is_exports(&*member.obj) {
            self.stats.is_cjs = 1;
            if !self.is_static_prop(&member.prop) {
                self.stats.non_static_exports = 1;
                // let start = self.source_map.lookup_char_pos(member.span.lo);
                // println!(
                //     "NON STATIC {}:{}:{}",
                //     start.file.name,
                //     start.line,
                //     start.col_display + 1
                // );
            }
            member.prop.visit_with(self);
        } else {
            member.visit_children_with(self);
        }
    }

    fn visit_ident(&mut self, id: &swc_core::ecma::ast::Ident) {
        // If `exports` or `module` was seen outside a member expresion, it's non-static.
        // e.g. `someFunction(exports)` could mutate it.
        if (id.sym == "exports" || id.sym == "module") && id.ctxt.has_mark(self.unresolved_mark) {
            self.stats.is_cjs = 1;
            self.stats.non_static_exports = 1;
            // let start = self.source_map.lookup_char_pos(id.span.lo);
            // println!(
            //     "NON STATIC {}:{}:{}",
            //     start.file.name,
            //     start.line,
            //     start.col_display + 1
            // );
        }
    }
}

impl Analyzer {
    fn is_exports(&self, expr: &Expr) -> bool {
        match expr {
            Expr::Ident(id) => id.sym == "exports" && id.ctxt.has_mark(self.unresolved_mark),
            Expr::Member(member2) => self.is_module_exports(member2),
            _ => false,
        }
    }

    fn is_module_exports(&self, member: &MemberExpr) -> bool {
        matches!(&*member.obj, Expr::Ident(id) if id.sym == "module" && id.ctxt.has_mark(self.unresolved_mark))
            && matches!(&member.prop, MemberProp::Ident(id) if id.sym == "exports")
    }

    fn is_static_prop(&self, prop: &MemberProp) -> bool {
        match prop {
            MemberProp::Ident(_) => true,
            MemberProp::Computed(computed) => computed
                .expr
                .as_pure_string(&swc_core::ecma::utils::ExprCtx {
                    unresolved_ctxt: SyntaxContext::empty().apply_mark(self.unresolved_mark),
                    is_unresolved_ref_safe: false,
                })
                .is_known(),
            _ => false,
        }
    }
}
