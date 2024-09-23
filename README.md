# analyze-npm

A program to analyze the syntax used by top npm packages. This uses the list of top package collected by [LeoDog896/npm-rank](https://github.com/LeoDog896/npm-rank/tree/main), and analyzes whether they use ESM or CommonJS, whether their CommonJS exports and requires are statically analyzable, etc.

## How to run

1. Run `node install.js` to generate the package.json with dependencies to install.
2. Run a package manager like `npm`. I used `bun install`.
3. Run `cargo run --release` to run the analysis. There may be a few errors for things that couldn't be parsed.

## Results

As of 2024-09-22, these are the results:

* Packages analyzed: 9800
* Total files: 101032
* Files with ESM: 51751
* Files with dynamic import: 276
* Files with CommonJS: 49139
* Files with non-static exports: 20370
* Files with non-static requires: 360
* Errors: 15
