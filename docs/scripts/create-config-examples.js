const fs = require('fs');
const cueJsonOutput = "data/docs.json";
const chalk = require('chalk');
const TOML = require('@iarna/toml');
const YAML = require('yaml');

try {
  console.log(chalk.blue("Creating example configurations for all Vector components..."));

  const data = fs.readFileSync(cueJsonOutput, 'utf8');
  const docs = JSON.parse(data);
  const components = docs.components;

  // Sources, transforms, sinks
  for (const kind in components) {
    console.log(chalk.blue(`Creating examples for ${kind}...`));

    const componentsOfKind = components[kind];

    // Specific components
    for (const componentType in componentsOfKind) {
      const component = componentsOfKind[componentType];
      const configuration = component.configuration;

      var exampleConfig = {
        kind: kind,
        "type": componentType
      };

      var requiredParams = [];
      var required = {};

      // Config parameters
      for (const paramName in configuration) {
        const param = configuration[paramName];

        if (param.required) {
          requiredParams.push(paramName);
        }
      }

      // Remove the "type" param from required, which is added elsewhere
      requiredParams = requiredParams.filter(p => p != "type");

      requiredParams.forEach((p) => {
        const param = configuration[p];
        Object.keys(param.type).forEach((key) => {
          const examples = param.type[key].examples;

          if (examples != null && examples.length > 0) {
            required[p] = examples[0];
          }
        });
      });

      const keyName = `my_${kind.substring(0, kind.length - 1)}_id`;

      var example = {
        [kind]: {
          [keyName]: {
            "type": componentType,
            ...required,
          }
        }
      };

      if (['sinks', 'transforms'].includes(kind)) {
        example[kind][keyName]['inputs'] = ['my-source-or-transform-id']
      }

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

  console.log(chalk.green("Success. Finished generating examples for all components."));
  console.log(chalk.blue(`Writing generated examples as JSON to ${cueJsonOutput}...`));

  fs.writeFileSync(cueJsonOutput, JSON.stringify(docs), 'utf8');

  console.log(chalk.green(`Success. Finished writing example configs to ${cueJsonOutput}.`));
} catch (err) {
  console.error(err);
}
