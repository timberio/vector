const fs = require('fs');
const docsFile = "data/docs.json";
const chalk = require('chalk');
const TOML = require('@iarna/toml');
const YAML = require('yaml');

try {
  chalk.blue("Creating example configurations for all Vector components");

  const data = fs.readFileSync(docsFile, 'utf8');
  const docs = JSON.parse(data);
  const components = docs.components;

  // Sources, transforms, sinks
  for (const kind in components) {
    const componentsOfKind = components[kind];

    // Specific components
    for (const componentType in componentsOfKind) {
      const component = componentsOfKind[componentType];
      var config = {
        kind: kind,
        "type": componentType
      };

      var required = {};

      // Config parameters
      for (const paramName in component.configuration) {
        const param = component.configuration[paramName];

        if (param.required) {
          required[paramName] = param;
        }
      }

      const keyName = `my_${kind.substring(0, kind.length - 1)}_id`;

      var example = {
        [kind]: {
          [keyName]: {
            "type": componentType,
          }
        }
      };

      const common = example;
      const advanced = example;

      docs['components'][kind][componentType]['example_configs'] = {
        common: {
          toml: TOML.stringify(common),
          yaml: `---\n${YAML.stringify(common)}`,
          json: JSON.stringify(common, null, 2),
        },
        advanced: {
          toml: TOML.stringify(advanced),
          yaml: `---\n${YAML.stringify(advanced)}`,
          json: JSON.stringify(advanced, null, 2)
        },
      };
    }
  }

  fs.writeFileSync(docsFile, JSON.stringify(docs), 'utf8');
} catch (err) {
  console.error(err);
}
