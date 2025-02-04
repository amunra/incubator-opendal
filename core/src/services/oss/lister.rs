// Licensed to the Apache Software Foundation (ASF) under one
// or more contributor license agreements.  See the NOTICE file
// distributed with this work for additional information
// regarding copyright ownership.  The ASF licenses this file
// to you under the Apache License, Version 2.0 (the
// "License"); you may not use this file except in compliance
// with the License.  You may obtain a copy of the License at
//
//   http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing,
// software distributed under the License is distributed on an
// "AS IS" BASIS, WITHOUT WARRANTIES OR CONDITIONS OF ANY
// KIND, either express or implied.  See the License for the
// specific language governing permissions and limitations
// under the License.

use std::sync::Arc;

use async_trait::async_trait;
use bytes::Buf;
use quick_xml::de;
use quick_xml::escape::unescape;
use serde::Deserialize;

use super::core::*;
use super::error::parse_error;
use crate::raw::*;
use crate::EntryMode;
use crate::Error;
use crate::ErrorKind;
use crate::Metadata;
use crate::Result;

pub struct OssLister {
    core: Arc<OssCore>,

    path: String,
    delimiter: &'static str,
    limit: Option<usize>,
    /// Filter results to objects whose names are lexicographically
    /// **equal to or after** startOffset
    start_after: Option<String>,
}

impl OssLister {
    pub fn new(
        core: Arc<OssCore>,
        path: &str,
        recursive: bool,
        limit: Option<usize>,
        start_after: Option<&str>,
    ) -> Self {
        let delimiter = if recursive { "" } else { "/" };
        Self {
            core,
            path: path.to_string(),
            delimiter,
            limit,
            start_after: start_after.map(String::from),
        }
    }
}

#[async_trait]
impl oio::PageList for OssLister {
    async fn next_page(&self, ctx: &mut oio::PageContext) -> Result<()> {
        let resp = self
            .core
            .oss_list_object(
                &self.path,
                &ctx.token,
                self.delimiter,
                self.limit,
                if ctx.token.is_empty() {
                    self.start_after.clone()
                } else {
                    None
                },
            )
            .await?;

        if resp.status() != http::StatusCode::OK {
            return Err(parse_error(resp).await?);
        }

        let bs = resp.into_body().bytes().await?;

        let output: ListBucketOutput = de::from_reader(bs.reader())
            .map_err(|e| Error::new(ErrorKind::Unexpected, "deserialize xml").set_source(e))?;

        ctx.done = !output.is_truncated;
        ctx.token = output.next_continuation_token.unwrap_or_default();

        for prefix in output.common_prefixes {
            let de = oio::Entry::new(
                &build_rel_path(&self.core.root, &prefix.prefix),
                Metadata::new(EntryMode::DIR),
            );
            ctx.entries.push_back(de);
        }

        for object in output.contents {
            if object.key.ends_with('/') {
                continue;
            }

            // exclude the inclusive start_after itself
            let path = &build_rel_path(&self.core.root, &object.key);
            if self.start_after.as_ref() == Some(path) {
                continue;
            }
            let mut meta = Metadata::new(EntryMode::FILE);

            meta.set_etag(&object.etag);
            meta.set_content_md5(object.etag.trim_matches('"'));
            meta.set_content_length(object.size);
            meta.set_last_modified(parse_datetime_from_rfc3339(object.last_modified.as_str())?);

            let rel = build_rel_path(&self.core.root, &object.key);
            let path = unescape(&rel)
                .map_err(|e| Error::new(ErrorKind::Unexpected, "excapse xml").set_source(e))?;
            let de = oio::Entry::new(&path, meta);
            ctx.entries.push_back(de);
        }

        Ok(())
    }
}

#[derive(Default, Debug, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
struct ListBucketOutput {
    prefix: String,
    max_keys: u64,
    encoding_type: String,
    is_truncated: bool,
    common_prefixes: Vec<CommonPrefix>,
    contents: Vec<Content>,
    key_count: u64,

    next_continuation_token: Option<String>,
}

#[derive(Default, Debug, Deserialize, PartialEq, Eq)]
#[serde(default, rename_all = "PascalCase")]
struct Content {
    key: String,
    last_modified: String,
    #[serde(rename = "ETag")]
    etag: String,
    size: u64,
}

#[derive(Default, Debug, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
struct CommonPrefix {
    prefix: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_list_output() {
        let bs = bytes::Bytes::from(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<ListBucketResult xmlns="https://doc.oss-cn-hangzhou.aliyuncs.com">
    <Name>examplebucket</Name>
    <Prefix></Prefix>
    <StartAfter>b</StartAfter>
    <MaxKeys>3</MaxKeys>
    <EncodingType>url</EncodingType>
    <IsTruncated>true</IsTruncated>
    <NextContinuationToken>CgJiYw--</NextContinuationToken>
    <Contents>
        <Key>b/c</Key>
        <LastModified>2020-05-18T05:45:54.000Z</LastModified>
        <ETag>"35A27C2B9EAEEB6F48FD7FB5861D****"</ETag>
        <Size>25</Size>
        <StorageClass>STANDARD</StorageClass>
        <Owner>
            <ID>1686240967192623</ID>
            <DisplayName>1686240967192623</DisplayName>
        </Owner>
    </Contents>
    <Contents>
        <Key>ba</Key>
        <LastModified>2020-05-18T11:17:58.000Z</LastModified>
        <ETag>"35A27C2B9EAEEB6F48FD7FB5861D****"</ETag>
        <Size>25</Size>
        <StorageClass>STANDARD</StorageClass>
        <Owner>
            <ID>1686240967192623</ID>
            <DisplayName>1686240967192623</DisplayName>
        </Owner>
    </Contents>
    <Contents>
        <Key>bc</Key>
        <LastModified>2020-05-18T05:45:59.000Z</LastModified>
        <ETag>"35A27C2B9EAEEB6F48FD7FB5861D****"</ETag>
        <Size>25</Size>
        <StorageClass>STANDARD</StorageClass>
        <Owner>
            <ID>1686240967192623</ID>
            <DisplayName>1686240967192623</DisplayName>
        </Owner>
    </Contents>
    <KeyCount>3</KeyCount>
</ListBucketResult>"#,
        );

        let out: ListBucketOutput = de::from_reader(bs.reader()).expect("must_success");

        assert!(out.is_truncated);
        assert_eq!(out.next_continuation_token, Some("CgJiYw--".to_string()));
        assert!(out.common_prefixes.is_empty());

        assert_eq!(
            out.contents,
            vec![
                Content {
                    key: "b/c".to_string(),
                    last_modified: "2020-05-18T05:45:54.000Z".to_string(),
                    etag: "\"35A27C2B9EAEEB6F48FD7FB5861D****\"".to_string(),
                    size: 25,
                },
                Content {
                    key: "ba".to_string(),
                    last_modified: "2020-05-18T11:17:58.000Z".to_string(),
                    etag: "\"35A27C2B9EAEEB6F48FD7FB5861D****\"".to_string(),
                    size: 25,
                },
                Content {
                    key: "bc".to_string(),
                    last_modified: "2020-05-18T05:45:59.000Z".to_string(),
                    etag: "\"35A27C2B9EAEEB6F48FD7FB5861D****\"".to_string(),
                    size: 25,
                }
            ]
        )
    }
}
