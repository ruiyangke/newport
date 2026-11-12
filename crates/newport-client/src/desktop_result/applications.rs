use super::{configuration, product};
use newport_protocol::desktop::applications::*;
use serde_json::{Value, json};
fn identity(value: Identity) -> Value {
    json!({"runtime":{"instance":value.runtime.instance,"buildSha256":value.runtime.build_sha256,"transportGeneration":value.runtime.transport_generation},"session":value.session,"worker":value.worker,"displayGeneration":value.display_generation})
}
fn application(value: Application) -> Value {
    json!({"display":identity(value.display),"request":value.request.get()})
}
fn outcome(value: Outcome) -> Value {
    match value {
        Outcome::Accepted(_) => json!({"state":"accepted"}),
        Outcome::Rejected(value) => json!({"state":"rejected","message":*value}),
        Outcome::Unknown(value) => json!({"state":"unknown","message":*value}),
    }
}
fn ending(value: Ending) -> Value {
    match value {
        Ending::Failed(value) => json!({"kind":"failed","detail":*value}),
        Ending::NoWindow(value) => json!({"kind":"noWindow","detail":value.detail}),
        Ending::HandedOff(_) => json!({"kind":"handedOff"}),
        Ending::DisplayStopped(_) => json!({"kind":"displayStopped"}),
        Ending::ShownElsewhere(value) => json!({"kind":"shownElsewhere","detail":value.detail}),
    }
}
fn gap(value: Gap) -> Value {
    let requirement = match value.requirement {
        Requirement::Dbus => "dbus",
        Requirement::Xwayland => "xwayland",
        Requirement::Fonts => "fonts",
        Requirement::Gles => "gles",
        Requirement::Portal => "portal",
        Requirement::Audio => "audio",
    };
    let mut row = match value.finding {
        Finding::Missing(_) => json!({"state":"missing"}),
        Finding::Unverified(_) => json!({"state":"unverified"}),
        Finding::Outdated(version) => json!({"state":"outdated","version":*version}),
        Finding::Inactive(reason) => {
            json!({"state":"inactive","reason":match *reason { Inactive::Root=>"root",Inactive::Stopped=>"stopped",Inactive::Failed=>"failed" }})
        }
    };
    row["requirement"] = json!(requirement);
    row
}
fn availability(value: Availability) -> Value {
    let elevation = match value.host.elevation {
        Elevation::Root => "root",
        Elevation::Sudo => "sudo",
        Elevation::Doas => "doas",
        Elevation::Unavailable => "unavailable",
    };
    let updater = value.host.updater.map(|value| match value {
        Updater::RpmOstree => "rpmOstree",
        Updater::TransactionalUpdate => "transactionalUpdate",
    });
    json!({"agent":value.agent,"legacyAgent":value.legacy_agent,"legacyRevision":value.legacy_revision,"legacyAgents":value.legacy_agents.into_iter().map(|value|json!({"revision":value.revision,"agent":value.agent,"builtAt":value.built_at})).collect::<Vec<_>>(),"system":value.system,"architecture":value.architecture,"gaps":value.gaps.into_iter().map(gap).collect::<Vec<_>>(),"host":{"elevation":elevation,"updater":updater,"userServices":value.host.user_services},"osRelease":value.os_release,"expectedConnection":configuration::server(value.expected_server),"bundledVariant":product(value.bundled_variant),"bundledRevision":value.bundled_revision,"total":value.total,"installedVariant":value.installed_variant.map(product),"revision":value.revision,"buildRelation":configuration::build_relation(value.build_relation),"installationError":value.installation_error,"selectedRuntime":value.selected_runtime.map(|value|json!({"buildSha256":value.build_sha256,"product":product(value.product)})),"agentError":value.agent_error,"automaticUpdate":value.automatic_update.map(configuration::update_outcome)})
}
pub(super) fn project(reply: Reply) -> Value {
    match reply {
        Reply::Status(value)=>availability(*value),
        Reply::Catalogue(value)=>json!({"apps":value.apps.into_iter().map(|value|json!({"id":value.id,"name":value.name,"description":value.description,"keywords":value.keywords,"icon":value.icon,"actions":value.actions.into_iter().map(|value|json!({"id":value.id,"name":value.name})).collect::<Vec<_>>()})).collect::<Vec<_>>()}),
        Reply::Display(value)=>json!({"shownOn":value.shown_on.map(|value|json!({"computer":value.computer,"session":value.session})),"applications":value.applications.into_iter().map(|value|json!({"appId":value.app_id,"windows":value.windows})).collect::<Vec<_>>()}),
        Reply::Show(_)=>Value::Null,
        Reply::Stop(value)=>json!({"stopped":value.stopped}),
        Reply::Launch(value)=>json!({"application":application(value.application),"outcome":outcome(value.outcome)}),
        Reply::States(value)=>Value::Array(value.observations.into_iter().map(|value|json!({"application":application(value.application),"state":value.state.map(|value|match value {State::Pending=>"pending",State::Opening=>"opening",State::Background=>"background",State::Running=>"running",State::Stopping=>"stopping",State::Ended=>"ended"}),"error":value.error,"ending":value.ending.map(ending)})).collect()),
    }
}
