use incurs::schema::FieldMeta;
use nb_api::NbError;
use serde_json::json;

use crate::handler;
use crate::spec::*;

const ACCESS: [&str; 2] = ["viewer", "editor"];

pub fn specs() -> Vec<Spec> {
    vec![
        Spec::new(
            "share status",
            Effect::R,
            "Link scope, access and collaborators",
            status,
        )
        .arg(note_arg()),
        Spec::new(
            "share collaborators",
            Effect::R,
            "Collaborators on a note",
            collaborators,
        )
        .arg(note_arg()),
        Spec::new(
            "share link",
            Effect::D,
            "Set link scope/access (widens exposure)",
            link,
        )
        .arg(note_arg())
        .opt(FieldMeta {
            required: true,
            ..choice(
                "scope",
                &["restricted", "public"],
                None,
                "public = anyone with the link.",
            )
        })
        .opt(choice(
            "access",
            &ACCESS,
            Some("viewer"),
            "Link access level.",
        )),
        Spec::new("share revoke", Effect::W, "Turn the share link off", revoke).arg(note_arg()),
        Spec::new(
            "share user",
            Effect::D,
            "Change or remove a collaborator's access",
            user,
        )
        .arg(note_arg())
        .arg(req("user_id", "User id (from share status)."))
        .opt(choice("access", &ACCESS, None, "New access level."))
        .opt(flag("remove", "Remove the user's access.")),
        Spec::new(
            "share answer",
            Effect::W,
            "Grant or deny an access request",
            answer,
        )
        .arg(req("request_id", "Request id (from share status)."))
        .arg(req_choice("decision", &["grant", "deny"], "Grant or deny."))
        .opt(choice(
            "access",
            &ACCESS,
            Some("viewer"),
            "Access level to grant.",
        )),
        Spec::new(
            "share search-users",
            Effect::R,
            "Search people in your business who can be invited (business accounts)",
            search_users,
        )
        .arg(req("query", "Email text to search for."))
        .opt(text("note", "Only people invitable to this note."))
        .opt(text(
            "workspace",
            "Only people invitable to this workspace id.",
        ))
        .opt(alias(num_def("limit", 10.0, "Max results."), 'n')),
        Spec::new(
            "share invite",
            Effect::W,
            "Email invitations (Notability emails each invitee)",
            invite,
        )
        .cli_only()
        .arg(note_arg())
        .arg(req_multi("emails", "Invitee email addresses."))
        .opt(choice("access", &ACCESS, Some("viewer"), "Access level."))
        .opt(alias(
            cli_name(
                flag(
                    "send",
                    "Required: confirms that Notability will email each invitee.",
                ),
                "yes",
            ),
            'y',
        )),
    ]
}

fn access(c: &Ctx) -> String {
    c.os("access").unwrap_or_else(|| "viewer".into())
}

handler!(status, |c| {
    let cl = c.client()?;
    cl.share_status(&c.s("note")?).await
});

handler!(collaborators, |c| {
    let cl = c.client()?;
    cl.collaborators(&c.s("note")?).await
});

handler!(link, |c| {
    let cl = c.client()?;
    cl.share_link(&c.s("note")?, &c.s("scope")?, &access(&c))
        .await
});

handler!(revoke, |c| {
    let cl = c.client()?;
    cl.share_link(&c.s("note")?, "restricted", "viewer").await
});

handler!(user, |c| {
    let acc = c.os("access");
    if acc.is_some() == c.b("remove") {
        return Err(NbError::usage("pass --access viewer|editor or --remove"));
    }
    let cl = c.client()?;
    cl.share_user(&c.s("note")?, &c.s("user_id")?, acc.as_deref())
        .await
});

handler!(answer, |c| {
    let cl = c.client()?;
    cl.share_answer(
        &c.s("request_id")?,
        c.s("decision")? == "grant",
        &access(&c),
    )
    .await
});

handler!(invite, |c| {
    if !c.b("send") {
        return Err(NbError::usage(
            "inviting sends email from Notability; re-run with --yes to confirm",
        ));
    }
    let cl = c.client()?;
    let r = cl
        .share_invite(&c.s("note")?, &c.list("emails"), &access(&c))
        .await?;
    Ok(json!(r))
});

handler!(search_users, |c| {
    let cl = c.client()?;
    let q = c.s("query")?;
    if q.trim().is_empty() {
        return Err(NbError::usage("query is empty"));
    }
    cl.invitable_users(
        q.trim(),
        c.ou("limit")?.unwrap_or(10).clamp(1, 50),
        c.os("note").as_deref(),
        c.os("workspace").as_deref(),
    )
    .await
});
