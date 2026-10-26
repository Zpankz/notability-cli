//! GraphQL reads used by the CLI.

use serde_json::{Value, json};

use crate::error::{Kind, Result};
use crate::http::Http;

const ME: &str = "query getMe { me { id email name isAiEnabled useType profile { screenname }
              loginProviders { provider verified } } }";
const SUBSCRIPTION: &str = "query getSubscriptionInfoForUser { subscriptionInfo: getSubscriptionInfoForUser {
              isClassicUser subscriptionTier subscriptionSource trialLengthInDays trialExpirationDate
              subscriptionRenewalDate subscriptionExpirationDate } }";
const CONNECTED: &str =
    "query getConnectedOAuthAccounts { me { connectedOAuthAccounts { id provider
              providerAccountEmail grantedScopes healthStatus createdAt } } }";
const STUDY: &str = "query getStudyMaterialsBatch($p: GetStudyMaterialsBatchParams!) {
          getStudyMaterialsBatch(getStudyMaterialsBatchParams: $p) { id promptGroup language isServerManaged
            asrJobs { sha512Hash } quizJobs { id type promptKey status errorReason userUploaded }
            noteQuizJobs { id userUploaded } summaryJobs { id status errorReason }
            summaryOfSummaries { id status promptKey errorReason } takeawaysSummary { id status promptKey errorReason } } }";
const ASR: &str = "query getASRJob($p: GetASRJobParams!) { getASRJob(getASRJobParams: $p) { success errorCode
              job { status errorReason results { segments { text startTime endTime speaker } } } } }";
const SUMMARY: &str = "query getSummaryJobContent($p: GetSummaryJobContentParams!) {
              getSummaryJobContent(getSummaryJobContentParams: $p) }";
const QUIZ: &str = "query getQuizJobContent($p: GetQuizJobContentParams!) {
              getQuizJobContent(getQuizJobContentParams: $p) }";
const QUIZ_SESSIONS: &str = "query getQuizSessions($p: GetQuizSessionsParams!) { getQuizSessions(getQuizSessionsParams: $p) {
              id noteId mode numQuestions numAnswered numCorrect numIncorrect numSkipped createdAt updatedAt completedAt } }";
const FLASH_DUE: &str = "query flashcardDueCounts($p: FlashcardDueCountsParams) {
              flashcardDueCounts(flashcardDueCountsParams: $p) { noteId dueCount truncated } }";
const THREADS: &str = "query getChatbotThreads($p: GetChatbotThreadsParams!) { getChatbotThreads(getChatbotThreadsParams: $p) {
              id title exchangeCount createdAt updatedAt } }";
const DELETE_THREAD: &str = "mutation deleteChatbotThread($p: DeleteChatbotThreadParams!) {
              deleteChatbotThread(deleteChatbotThreadParams: $p) }";
const OAUTH_TOKEN: &str =
    "mutation requestOAuthAccessToken($params: RequestOAuthAccessTokenParams!) {
              requestOAuthAccessToken(params: $params) { accessToken expiresAt grantedScopes } }";

fn list(v: Value) -> Vec<Value> {
    match v {
        Value::Array(a) => a,
        _ => Vec::new(),
    }
}

impl Http {
    pub async fn me(&self) -> Result<Value> {
        self.gql_field(ME, json!({}), "me").await
    }

    pub async fn subscription(&self) -> Result<Value> {
        self.gql_field(SUBSCRIPTION, json!({}), "subscriptionInfo")
            .await
    }

    pub async fn connected_accounts(&self) -> Result<Vec<Value>> {
        let me = self.gql_field(CONNECTED, json!({}), "me").await?;
        Ok(list(
            me.get("connectedOAuthAccounts")
                .cloned()
                .unwrap_or_default(),
        ))
    }

    /// `None` when the note has no study-materials batch yet.
    pub async fn study_materials(&self, note_id: &str) -> Result<Option<Value>> {
        match self
            .gql_field(
                STUDY,
                json!({"p": {"clientNoteId": note_id}}),
                "getStudyMaterialsBatch",
            )
            .await
        {
            Ok(v) if v.is_null() => Ok(None),
            Ok(v) => Ok(Some(v)),
            Err(e) if e.kind == Kind::NotFound => Ok(None),
            Err(e) => {
                let k = e.key.as_deref().unwrap_or_default().to_lowercase();
                if k.contains("notfound") || (k.starts_with("no") && k.ends_with("found")) {
                    Ok(None)
                } else {
                    Err(e)
                }
            }
        }
    }

    pub async fn transcript(&self, sha512: &str) -> Result<Value> {
        self.gql_field(ASR, json!({"p": {"sha512Hash": sha512}}), "getASRJob")
            .await
    }

    pub async fn summary_content(&self, job_id: &str) -> Result<Value> {
        self.gql_field(
            SUMMARY,
            json!({"p": {"summaryJobId": job_id}}),
            "getSummaryJobContent",
        )
        .await
    }

    pub async fn quiz_content(&self, job_id: &str) -> Result<Value> {
        self.gql_field(
            QUIZ,
            json!({"p": {"quizJobId": job_id}}),
            "getQuizJobContent",
        )
        .await
    }

    pub async fn quiz_sessions(
        &self,
        note_id: Option<&str>,
        limit: usize,
        status: &str,
    ) -> Result<Vec<Value>> {
        let mut p = json!({"limit": limit, "status": status});
        if let Some(n) = note_id {
            p["noteId"] = json!(n);
        }
        Ok(list(
            self.gql_field(QUIZ_SESSIONS, json!({"p": p}), "getQuizSessions")
                .await?,
        ))
    }

    pub async fn flashcard_due(&self, note_ids: &[String]) -> Result<Vec<Value>> {
        let p = if note_ids.is_empty() {
            json!({})
        } else {
            json!({"noteIds": note_ids})
        };
        Ok(list(
            self.gql_field(FLASH_DUE, json!({"p": p}), "flashcardDueCounts")
                .await?,
        ))
    }

    pub async fn chat_threads(&self, limit: usize, offset: usize) -> Result<Vec<Value>> {
        Ok(list(
            self.gql_field(
                THREADS,
                json!({"p": {"limit": limit, "offset": offset}}),
                "getChatbotThreads",
            )
            .await?,
        ))
    }

    pub async fn chat_delete(&self, thread_id: &str) -> Result<bool> {
        let v = self
            .gql_field(
                DELETE_THREAD,
                json!({"p": {"chatbotThreadId": thread_id}}),
                "deleteChatbotThread",
            )
            .await?;
        Ok(v.as_bool().unwrap_or(!v.is_null()))
    }

    pub async fn oauth_token(&self, account: &Value, scope: &str) -> Result<Value> {
        let params = json!({"connectedAccountId": account["id"], "requiredScopes": [scope]});
        self.gql_field(
            OAUTH_TOKEN,
            json!({"params": params}),
            "requestOAuthAccessToken",
        )
        .await
    }
}
