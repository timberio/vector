import React from 'react';

import Avatar from '@site/src/components/Avatar';
import BlogPostTags from '@site/src/components/BlogPostTags';
import Layout from '@theme/Layout';
import BlogPostItem from '@theme/BlogPostItem';
import BlogPostPaginator from '@theme/BlogPostPaginator';
import MailingListForm from '@site/src/components/MailingListForm';
import MDXComponents from '@theme/MDXComponents';
import {MDXProvider} from '@mdx-js/react';

import classnames from 'classnames';
import dateFormat from 'dateformat';
import {enrichTags} from '@site/src/exports/tags';
import readingTime from 'reading-time';
import styles from './styles.module.css';

function BlogPostPage(props) {
  const {content: BlogPostContents} = props;
  const {frontMatter, metadata} = BlogPostContents;
  const {author_id, title} = frontMatter;
  const {dateString, tags} = metadata;
  const readingStats = readingTime(BlogPostContents.toString());
  const date = Date.parse(dateString);
  const domainTag = enrichTags(tags).find(tag => tag.category == 'domain');
  const domain = domainTag ? domainTag.value : null;

  return (
    <Layout title={metadata.title} description={metadata.description}>
      <article className={styles.blogPost}>
        <header className={classnames('hero', styles.header)}>
          <div className={classnames('domain-bg', `domain-bg--${domain}`, styles.headerBackground)}></div>
          <div className={classnames('container', styles.headerContainer)}>
            <div>
              <Avatar id={author_id} size="lg" nameSuffix={` / ${dateFormat(date, "mmm dS, yyyy")} / ${readingStats.text}`} subTitle={false} vertical={true} />
              <h1>{title}</h1>
              <div className={styles.headerTags}>
                <BlogPostTags tags={tags} />
              </div>
            </div>
          </div>
        </header>
        <div className="container container--narrow container--bleed margin-vert--xl">
          <section className="markdown">
            <MDXProvider components={MDXComponents}><BlogPostContents /></MDXProvider>
          </section>
          <section className="panel bleed" style={{textAlign: 'center'}}>
            <MailingListForm size="lg" />
          </section>
          {(metadata.nextItem || metadata.prevItem) && (
            <div className="bleed margin-vert--xl">
              <BlogPostPaginator
                nextItem={metadata.nextItem}
                prevItem={metadata.prevItem}
              />
            </div>
          )}
        </div>
      </article>
    </Layout>
  );
}

export default BlogPostPage;
