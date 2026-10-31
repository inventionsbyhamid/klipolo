//! What an export's captions freeze from: what was said in the project's files, as the preview has it. An export reads
//! finished transcripts only and never starts one: a file still being transcribed is left out, never frozen as if its
//! words so far were all of them. The engine burns the captions in from these words, page by page, as the preview
//! draws them.

use std::sync::Arc;

use klipolo_model::project::Project;
use klipolo_model::speech::Speech;

/// `speech` with only `project`'s files, and of their transcripts only the complete ones (their sound levels too, for
/// the music's ducking). O(files x assets).
pub fn export_speech(speech: &Speech, project: &Project) -> Arc<Speech> {
    let mine = |asset: &String| project.asset(asset).is_some();
    Arc::new(Speech {
        transcripts: speech
            .transcripts
            .iter()
            .filter(|(asset, t)| t.complete && mine(asset))
            .map(|(asset, t)| (asset.clone(), Arc::clone(t)))
            .collect(),
        levels: speech
            .levels
            .iter()
            .filter(|(asset, _)| mine(asset))
            .map(|(asset, l)| (asset.clone(), Arc::clone(l)))
            .collect(),
        topics: Default::default(),
    })
}

#[cfg(test)]
mod tests {
    use klipolo_model::speech::Transcript;

    use super::*;

    #[test]
    fn an_export_freezes_only_the_finished_transcripts_of_its_own_files() {
        let (project, speech) =
            klipolo_engine::test_support::captions_project("caption.bold", &[("Hello", 1.0, 1.4)]);
        let finished = speech.transcripts["v"].clone();
        let mut heard = speech.clone();
        // A file still being transcribed, and one another project uses.
        let mut half: Transcript = (*finished).clone();
        half.complete = false;
        let mut with_half = project.clone();
        let mut second = with_half.assets[0].clone();
        second.id = "half".into();
        second.path = "/media/half".into();
        with_half.assets.push(second);
        heard.transcripts.insert("half".into(), Arc::new(half));
        heard
            .transcripts
            .insert("elsewhere".into(), finished.clone());
        let frozen = export_speech(&heard, &with_half);
        let mut kept: Vec<&str> = frozen.transcripts.keys().map(String::as_str).collect();
        kept.sort_unstable();
        assert_eq!(kept, ["v"], "finished, and of this project's files");
        assert_eq!(
            frozen.transcripts["v"].words, finished.words,
            "its words as they are"
        );
        assert!(
            export_speech(&Speech::default(), &project)
                .transcripts
                .is_empty(),
            "none: no captions"
        );
    }
}
