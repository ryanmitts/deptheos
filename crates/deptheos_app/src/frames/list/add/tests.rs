use super::*;
use crate::words::explain::Platform;

fn stack(n: u64) -> StackId {
    StackId::parse(&format!("{n:032x}")).unwrap()
}

fn list(files: &[&str]) -> List {
    let mut list = List::default();
    list.add_files_unsorted(files.iter().map(PathBuf::from).collect());
    list
}

fn read(files: &[&str]) -> Expanded {
    Expanded { files: files.iter().map(PathBuf::from).collect(), unreadable: Vec::new() }
}

/// Listed, listed for, on screen, running, its list, it has renders, from;
/// what it comes to.
type Case<'a> = (&'a [&'a str], &'a StackId, &'a StackId, bool, &'a List, bool, AddedFrom, Outcome);

#[test]
fn what_a_listing_comes_to() {
    let (a, b) = (stack(1), stack(2));
    let empty = list(&[]);
    let some = list(&["/f/1.jpg"]);
    let add = |files: &[&str], offer| Outcome::Add {
        files: files.iter().map(PathBuf::from).collect(),
        offer,
    };
    let cases: [Case; 9] = [
        // Into an empty stack: added, nothing offered.
        (&["/f/1.jpg"], &a, &a, false, &empty, false, AddedFrom::Window, add(&["/f/1.jpg"], false)),
        // Into one with frames: New Stack Instead; those listed already
        // and doubles left out.
        (
            &["/f/1.jpg", "/f/2.jpg", "/f/2.jpg"],
            &a,
            &a,
            false,
            &some,
            false,
            AddedFrom::Window,
            add(&["/f/2.jpg"], true),
        ),
        // Handed over by the system into an emptied stack with renders.
        (&["/g/1.jpg"], &a, &a, false, &empty, true, AddedFrom::Outside, add(&["/g/1.jpg"], true)),
        (&["/g/1.jpg"], &a, &a, false, &empty, true, AddedFrom::Window, add(&["/g/1.jpg"], false)),
        // All there already, or nothing at all.
        (&["/f/1.jpg"], &a, &a, false, &some, false, AddedFrom::Window, Outcome::Nothing),
        (&[], &a, &a, false, &empty, false, AddedFrom::Window, Outcome::Nothing),
        // Another stack came on screen, or a stack started, meanwhile.
        (
            &["/f/2.jpg"],
            &a,
            &b,
            false,
            &some,
            false,
            AddedFrom::Window,
            Outcome::Refused(Refused::OtherStack),
        ),
        (&[], &a, &b, true, &empty, false, AddedFrom::Window, Outcome::Nothing),
        (
            &["/f/2.jpg"],
            &a,
            &a,
            true,
            &some,
            false,
            AddedFrom::Window,
            Outcome::Refused(Refused::Stacking),
        ),
    ];
    for (listed, into, on, running, list, has_renders, from, outcome) in cases {
        let now = OnScreen { stack: on, running, list, has_renders };
        assert_eq!(plan(read(listed), into, now, from).outcome, outcome, "{listed:?} {from:?}");
    }
}

#[test]
fn an_unreadable_folder_is_kept_whatever_else_happens() {
    let a = stack(1);
    let empty = list(&[]);
    let denied = std::io::Error::from(std::io::ErrorKind::PermissionDenied);
    let gone = std::io::Error::from(std::io::ErrorKind::NotFound);
    let listed = Expanded {
        files: Vec::new(),
        unreadable: vec![(PathBuf::from("/cards/DCIM"), denied), (PathBuf::from("/x/old"), gone)],
    };
    let now = OnScreen { stack: &a, running: false, list: &empty, has_renders: false };
    let plan = plan(listed, &a, now, AddedFrom::Window);
    assert_eq!(plan.outcome, Outcome::Nothing);
    let words = Words::english(Platform::Desktop);
    let leads: Vec<String> =
        plan.unreadable.iter().map(|(f, e)| unreadable_lead(f, e, &words)).collect();
    assert_eq!(leads, ["Deptheos isn't allowed to read \"DCIM\".", "\"old\" couldn't be read."]);
}

#[test]
fn the_notices() {
    let words = Words::english(Platform::Ios);
    assert_eq!(added_text(12, "canon1", &words), "Added 12 frames to canon1.");
    assert_eq!(added_text(1, "canon1", &words), "Added 1 frame to canon1.");
    assert!(nothing_text(&words).starts_with("No frames were added."));
    assert!(Refused::OtherStack.text(&words).contains("another stack was opened"));
}
