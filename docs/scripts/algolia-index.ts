import chalk from "chalk";
import cheerio from "cheerio";
import { Element } from "domhandler";
import dotEnv from "dotenv-defaults";
import fs from "fs";
import glob from "glob-promise";
import path from "path";

dotEnv.config();

// Types
type Payload = {
  level: number;
  domId?: string;
  tagName: string;
  content: string;
}[];

type AlgoliaRecord = {
  objectID: string;
  pageTitle: string;
  pageUrl: string;
  itemUrl: string;
  level: number;
  title: string;
  hierarchy: string[];
  tags: string[];
  ranking: number;
  section: string;
  content: string;
};

// Constants
const targetFile = "./public/search.json";

const DEBUG = process.env.DEBUG === "true" || false;
const publicPath = path.resolve(__dirname, "..", "public");
const tagHierarchy = {
  h1: 6,
  h2: 5,
  h3: 4,
  h4: 3,
  h5: 2,
  h6: 1,
  li: 1,
  p: 1,
};

function getPageUrl(file: string) {
  const filePath = file.split("public/")[1].split(path.sep).slice(0, -1);
  return `/${filePath.join("/")}`;
}

function getItemUrl(file: string, { level, domId }: Payload[0]) {
  const fileUrl = getPageUrl(file);

  if (level > 1 && level < 6 && !domId) {
    console.log(chalk.yellow(`Missing domId for level ${level}`));
    console.log(chalk.yellow(`File ${file}`));
  }

  return level > 1 && level < 6 && domId ? `${fileUrl}#${domId}` : fileUrl;
}

async function indexHTMLFiles(
  section: string,
  files: string[],
  ranking: number
): Promise<AlgoliaRecord[]> {
  const usedIds = {};
  const algoliaRecords: AlgoliaRecord[] = [];

  for (const file of files) {
    const html = fs.readFileSync(file, "utf-8");
    const $ = cheerio.load(html);
    const containers = $("#page-content");
    const pageTitle = $('meta[name="algolia:title"]').attr('content') || "";

    const pageTags = $('meta[name="keywords"]').attr('content')?.split(",") || [];

    // @ts-ignore
    $(".algolia-no-index").each((_, d) => $(d).remove());
    // @ts-ignore
    $(".highlight").each((_, d) => $(d).remove());
    const payload: Payload = [];
    const traverse = (node?: Element) => {
      if (!node) {
        return;
      }

      const level = tagHierarchy[node.tagName];

      if (level) {
        payload.push({
          level,
          domId: $(node).attr("id"),
          tagName: node.tagName,
          content: $(node)
            .text()
            .replace(/[\n\t]/g, " "),
        });
      }

      $(node)
        .children()
        .map((_, d) => traverse(d));
    };

    for (let i = 0; i < containers.length; i++) {
      traverse(containers.get(i) as Element);
    }

    let activeRecord: AlgoliaRecord | null = null;

    for (const item of payload) {
      const pageUrl = getPageUrl(file);
      const itemUrl = getItemUrl(file, item);

      if (!activeRecord) {
        activeRecord = {
          objectID: itemUrl,
          pageTitle,
          pageUrl,
          itemUrl,
          level: item.level,
          title: item.content,
          section,
          ranking,
          hierarchy: [],
          tags: pageTags,
          content: "",
        };
      } else if (item.level === 1) {
        if (activeRecord.content) {
          activeRecord.content += " ";
        }

        activeRecord.content += item.content;
      } else if (item.level < activeRecord.level) {
        algoliaRecords.push({ ...activeRecord });

        activeRecord = {
          objectID: itemUrl,
          pageTitle,
          pageUrl,
          itemUrl,
          level: item.level,
          title: item.content,
          section,
          ranking,
          hierarchy: [...activeRecord.hierarchy, activeRecord.title],
          tags: pageTags,
          content: "",
        };
      } else {
        algoliaRecords.push({ ...activeRecord });
        const tagCount = activeRecord.hierarchy.length;
        const levelDiff = item.level - activeRecord.level;
        const lastIndex = tagCount - levelDiff;

        activeRecord = {
          objectID: itemUrl,
          pageTitle,
          pageUrl,
          itemUrl,
          level: item.level,
          title: item.content,
          section,
          ranking,
          hierarchy: [...activeRecord.hierarchy.slice(0, lastIndex)],
          tags: pageTags,
          content: "",
        };
      }

      if (activeRecord) {
        activeRecord.title = activeRecord.title.trim();
        activeRecord.hierarchy.map((item) => item.trim());

        algoliaRecords.push({ ...activeRecord });
      }

      for (const rec of algoliaRecords) {
        if (usedIds[rec.objectID]) {
          // The objectID is the url of the section of the page that the record covers.
          // If you have a duplicate here somehow two records point to the same thing.

          if (DEBUG) {
            console.log(chalk.yellow(`Duplicate ID for ${rec.objectID}`));
            console.log(JSON.stringify(rec, null, 2));
          }
        }

        usedIds[rec.objectID] = true;

        if (rec.level > 1 && rec.level < 6 && rec.hierarchy.length == 0) {
          // The h2 -> h5 should have a set of tags that are the "path" within the file.
          if (DEBUG) {
            console.log(chalk.yellow("Found h2 -> h5 with no tags."));
            console.log(JSON.stringify(rec, null, 2));
          }
        }
      }
    }
  }

  console.log(
    chalk.green(`Success. Updated records for ${files.length} file(s).`)
  );

  return algoliaRecords;
}

async function buildIndex() {
  var allRecords: AlgoliaRecord[] = [];

  const appId = process.env.ALGOLIA_APP_ID || "";
  const adminPublicKey = process.env.ALGOLIA_ADMIN_KEY || "";

  console.log(`Building Vector search index`);

  let files = await glob(`${publicPath}/docs/about/**/**.html`);
  console.log(chalk.blue("Indexing docs/about..."));
  let r1 = await indexHTMLFiles("Docs", files, 50);
  allRecords.push(...r1);

  files = await glob(`${publicPath}/docs/administration/**/**.html`);
  console.log(chalk.blue("Indexing docs/administration..."));
  let r2 = await indexHTMLFiles("Docs", files, 50);
  allRecords.push(...r2);

  files = await glob(`${publicPath}/docs/reference/**/**.html`);
  console.log(chalk.blue("Indexing docs/reference..."));
  let r3 = await indexHTMLFiles("Docs", files, 50);
  allRecords.push(...r3);

  files = await glob(`${publicPath}/docs/setup/**/**.html`);
  console.log(chalk.blue("Indexing docs/setup..."));
  let r4 = await indexHTMLFiles("Docs", files, 50);
  allRecords.push(...r4);

  files = await glob(`${publicPath}/guides/advanced/**/**.html`);
  console.log(chalk.blue("Indexing guides/advanced..."));
  let r5 = await indexHTMLFiles("Advanced guides", files, 40);
  allRecords.push(...r5);

  files = await glob(`${publicPath}/guides/level-up/**/**.html`);
  console.log(chalk.blue("Indexing guides/level-up..."));
  let r6 = await indexHTMLFiles("Level up guides", files, 40);
  allRecords.push(...r6);

  console.log(chalk.green(`Success. ${allRecords.length} records have been successfully indexed.`));
  console.log(chalk.blue(`Writing final index JSON to ${targetFile}...`));

  fs.writeFile(targetFile, JSON.stringify(allRecords), () => {
    console.log(chalk.green(`Success. Wrote final index JSON to ${targetFile}.`));
  });
}

buildIndex().catch((err) => {
  console.trace(chalk.yellow(err));
});
