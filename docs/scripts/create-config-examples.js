const fs = require('fs');
const docsFile = "data/docs.json";
const TOML = require('@iarna/toml');
const YAML = require('yaml');

try {
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

      var common = {};
      common[kind] = {};

      const keyName = `my_${kind.substring(0, kind.length - 1)}_id`;

      common[kind][keyName] = {
        "type": componentType,
      };

      const commonToml = TOML.stringify(common);
      const commonYaml = `---\n${YAML.stringify(common)}`;
      const commonJson = JSON.stringify(common);

      console.log(commonYaml);

      docs['components'][kind][componentType]['example_configs'] = {};
      docs['components'][kind][componentType]['example_configs']['toml'] = {};
      docs['components'][kind][componentType]['example_configs']['toml']['common'] = commonToml;

      docs['components'][kind][componentType]['example_configs']['yaml'] = {};
      docs['components'][kind][componentType]['example_configs']['yaml']['common'] = commonYaml;

      docs['components'][kind][componentType]['example_configs']['json'] = {};
      docs['components'][kind][componentType]['example_configs']['json']['common'] = commonJson;
    }
  }

  fs.writeFileSync(docsFile, JSON.stringify(docs), 'utf8');
} catch (err) {
  console.error(err);
}
