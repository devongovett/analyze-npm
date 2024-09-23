const fs = require('fs');

let data = JSON.parse(fs.readFileSync('data.json', 'utf8'));

let pkgJSON = {
  name: 'analyze-npm',
  dependencies: {}
};

// Some error during install somewhere in the skipped range.
for (let pkg of data.slice(0, 3800).concat(data.slice(4800))) {
  if (pkg === 'canvas') continue;
  pkgJSON.dependencies[pkg.name] = pkg.version;
}

console.log(Object.keys(pkgJSON.dependencies).length)
fs.writeFileSync('package.json', JSON.stringify(pkgJSON, false, 2));
