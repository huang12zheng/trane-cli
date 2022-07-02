//! Module containing the state of the application.
use anyhow::{anyhow, ensure, Result};
use chrono::Utc;
use trane::{
    blacklist::Blacklist,
    course_library::CourseLibrary,
    data::{
        filter::{FilterOp, FilterType, KeyValueFilter, MetadataFilter, UnitFilter},
        ExerciseManifest, MasteryScore, UnitType,
    },
    filter_manager::FilterManager,
    graph::DebugUnitGraph,
    scheduler::ExerciseScheduler,
    Trane,
};

use crate::cli::KeyValue;
use crate::display::{DisplayAnswer, DisplayAsset, DisplayExercise};

/// Stores the app and its configuration.
#[derive(Default)]
pub(crate) struct TraneApp {
    /// The instance of the Trane library.
    trane: Option<Trane>,

    /// The filter used to select exercises.
    filter: Option<UnitFilter>,

    /// The current batch of exercises.
    batch: Vec<(String, ExerciseManifest)>,

    /// The index of the current exercise in the batch.
    batch_index: usize,
}

impl TraneApp {
    /// Returns the current exercise.
    fn current_exercise(&self) -> Result<(String, ExerciseManifest)> {
        self.batch
            .get(self.batch_index)
            .cloned()
            .ok_or_else(|| anyhow!("cannot get current exercise"))
    }

    /// Returns the current exercise's course ID.
    fn current_exercise_course(&self) -> Result<String> {
        ensure!(self.trane.is_some(), "no Trane instance is open");

        let (_, manifest) = self.current_exercise()?;
        Ok(manifest.course_id)
    }

    /// Returns the current exercise's lesson ID.
    fn current_exercise_lesson(&self) -> Result<String> {
        ensure!(self.trane.is_some(), "no Trane instance is open");

        let (_, manifest) = self.current_exercise()?;
        Ok(manifest.lesson_id)
    }

    /// Resets the batch of exercises.
    fn reset_batch(&mut self) {
        self.batch.clear();
        self.batch_index = 0;
    }

    /// Adds the current exercise's course to the blacklist.
    pub fn blacklist_course(&mut self) -> Result<()> {
        ensure!(self.trane.is_some(), "no Trane instance is open");

        let course_id = self.current_exercise_course()?;
        self.trane.as_mut().unwrap().add_unit(&course_id)?;
        self.reset_batch();
        Ok(())
    }

    /// Adds the current exercise's lesson to the blacklist.
    pub fn blacklist_lesson(&mut self) -> Result<()> {
        ensure!(self.trane.is_some(), "no Trane instance is open");

        let lesson_id = self.current_exercise_lesson()?;
        self.trane.as_mut().unwrap().add_unit(&lesson_id)?;
        self.reset_batch();
        Ok(())
    }

    /// Adds the current exercise to the blacklist.
    pub fn blacklist_exercise(&mut self) -> Result<()> {
        ensure!(self.trane.is_some(), "no Trane instance is open");

        let (_, manifest) = self.current_exercise()?;
        self.trane.as_mut().unwrap().add_unit(&manifest.id)?;
        self.reset_batch();
        Ok(())
    }

    /// Adds the unit with the given ID to the blacklist.
    pub fn blacklist_unit(&mut self, unit_id: &str) -> Result<()> {
        ensure!(self.trane.is_some(), "no Trane instance is open");

        self.trane.as_mut().unwrap().add_unit(unit_id)?;
        self.reset_batch();
        Ok(())
    }

    /// Clears the unit filter if it's set.
    pub fn clear_filter(&mut self) {
        if self.filter.is_none() {
            return;
        }
        self.filter = None;
        self.reset_batch();
    }

    /// Displays the current exercise.
    pub fn current(&self) -> Result<()> {
        ensure!(self.trane.is_some(), "no Trane instance is open");

        let (_, manifest) = self.current_exercise()?;
        manifest.display_exercise()
    }

    /// Returns the given course ID or the current exercise's course ID if the given ID is empty.
    fn course_id_or_current(&self, course_id: &str) -> Result<String> {
        let current_course = self.current_exercise_course().unwrap_or_default();
        if course_id.is_empty() {
            if current_course.is_empty() {
                Err(anyhow!("cannot get current exercise"))
            } else {
                Ok(current_course)
            }
        } else {
            Ok(course_id.to_string())
        }
    }

    /// Returns the given lesson ID or the current exercise's lesson ID if the given ID is empty.
    fn lesson_id_or_current(&self, lesson_id: &str) -> Result<String> {
        let current_lesson = self.current_exercise_lesson().unwrap_or_default();
        if lesson_id.is_empty() {
            if current_lesson.is_empty() {
                Err(anyhow!("cannot get current exercise"))
            } else {
                Ok(current_lesson)
            }
        } else {
            Ok(lesson_id.to_string())
        }
    }

    /// Sets the filter to only show exercises from the given course.
    pub fn filter_course(&mut self, course_id: &str) -> Result<()> {
        ensure!(self.trane.is_some(), "no Trane instance is open");

        self.get_unit_uid(course_id)
            .map_err(|_| anyhow!("missing course with ID {}", course_id))?;
        let unit_type = self.get_unit_type(course_id)?;
        if unit_type != UnitType::Course {
            return Err(anyhow!("unit with ID {} is not a course", course_id));
        }

        self.filter = Some(UnitFilter::CourseFilter {
            course_id: course_id.to_string(),
        });
        self.reset_batch();
        Ok(())
    }

    /// Sets the filter to only show exercises from the given lesson.
    pub fn filter_lesson(&mut self, lesson_id: &str) -> Result<()> {
        ensure!(self.trane.is_some(), "no Trane instance is open");
        self.get_unit_uid(lesson_id)
            .map_err(|_| anyhow!("missing lesson with ID {}", lesson_id))?;
        let unit_type = self.get_unit_type(lesson_id)?;
        if unit_type != UnitType::Lesson {
            return Err(anyhow!("unit with ID {} is not a lesson", lesson_id));
        }

        self.filter = Some(UnitFilter::LessonFilter {
            lesson_id: lesson_id.to_string(),
        });
        self.reset_batch();
        Ok(())
    }

    /// Sets the filter to only show exercises which belong to any course or lesson with the given
    /// metadata.
    pub fn filter_metadata(
        &mut self,
        filter_op: FilterOp,
        lesson_metadata: &Option<Vec<KeyValue>>,
        course_metadata: &Option<Vec<KeyValue>>,
    ) -> Result<()> {
        let basic_lesson_filters: Option<Vec<KeyValueFilter>> =
            lesson_metadata.as_ref().map(|pairs| {
                pairs
                    .iter()
                    .map(|pair| KeyValueFilter::BasicFilter {
                        key: pair.key.clone(),
                        value: pair.value.clone(),
                        filter_type: FilterType::Include,
                    })
                    .collect()
            });
        let lesson_filter = basic_lesson_filters.map(|filters| KeyValueFilter::CombinedFilter {
            op: filter_op.clone(),
            filters,
        });

        let basic_course_filters: Option<Vec<KeyValueFilter>> =
            course_metadata.as_ref().map(|pairs| {
                pairs
                    .iter()
                    .map(|pair| KeyValueFilter::BasicFilter {
                        key: pair.key.clone(),
                        value: pair.value.clone(),
                        filter_type: FilterType::Include,
                    })
                    .collect()
            });
        let course_filter = basic_course_filters.map(|filters| KeyValueFilter::CombinedFilter {
            op: filter_op.clone(),
            filters,
        });

        self.filter = Some(UnitFilter::MetadataFilter {
            filter: MetadataFilter {
                op: filter_op,
                lesson_filter,
                course_filter,
            },
        });
        self.reset_batch();
        Ok(())
    }

    /// Returns the UID of the unit with the given ID.
    pub fn get_unit_uid(&self, unit_id: &str) -> Result<u64> {
        ensure!(self.trane.is_some(), "no Trane instance is open");

        self.trane
            .as_ref()
            .unwrap()
            .get_uid(unit_id)
            .ok_or_else(|| anyhow!("missing UID for unit with ID {}", unit_id))
    }

    /// Returns the type of the unit with the given ID.
    pub fn get_unit_type(&self, unit_id: &str) -> Result<UnitType> {
        ensure!(self.trane.is_some(), "no Trane instance is open");

        let unit_uid = self.get_unit_uid(unit_id)?;
        self.trane
            .as_ref()
            .unwrap()
            .get_unit_type(unit_uid)
            .ok_or_else(|| anyhow!("missing type for unit with ID {}", unit_id))
    }

    /// Returns the ID of the unit with the given UID.
    pub fn get_unit_id(&self, unit_uid: u64) -> Result<String> {
        ensure!(self.trane.is_some(), "no Trane instance is open");

        self.trane
            .as_ref()
            .unwrap()
            .get_id(unit_uid)
            .ok_or_else(|| anyhow!("missing ID for unit with UID {}", unit_uid))
    }

    /// Prints the a list of all the saved unit filters.
    pub fn list_filters(&self) -> Result<()> {
        ensure!(self.trane.is_some(), "no Trane instance is open");

        let filters = self.trane.as_ref().unwrap().list_filters();

        if filters.is_empty() {
            println!("No saved unit filters");
            return Ok(());
        }

        println!("Saved unit filters:");
        println!("ID\tDescription");
        for filter in filters {
            println!("{}\t{}", filter.0, filter.1);
        }
        Ok(())
    }

    /// Displays the next exercise.
    pub fn next(&mut self) -> Result<()> {
        ensure!(self.trane.is_some(), "no Trane instance is open");

        self.batch_index += 1;
        if self.batch.is_empty() || self.batch_index >= self.batch.len() {
            self.batch = self
                .trane
                .as_ref()
                .unwrap()
                .get_exercise_batch(self.filter.as_ref())?;
            self.batch_index = 0;
        }

        let (_, manifest) = self.current_exercise()?;
        manifest.display_exercise()
    }

    /// Opens the course library at the given path.
    pub fn open_library(&mut self, library_root: &str) -> Result<()> {
        let trane = Trane::new(library_root)?;
        self.trane = Some(trane);
        self.batch.drain(..);
        self.batch_index = 0;
        Ok(())
    }

    /// Assigns the given score to the current exercise.
    pub fn record_score(&mut self, score: u8) -> Result<()> {
        ensure!(self.trane.is_some(), "no Trane instance is open");

        let curr_exercise = self.current_exercise()?;
        let timestamp = Utc::now().timestamp();
        let mastery_score = match score {
            1 => Ok(MasteryScore::One),
            2 => Ok(MasteryScore::Two),
            3 => Ok(MasteryScore::Three),
            4 => Ok(MasteryScore::Four),
            5 => Ok(MasteryScore::Five),
            _ => Err(anyhow!("invalid score {}", score)),
        }?;
        self.trane.as_ref().unwrap().record_exercise_score(
            &curr_exercise.0,
            mastery_score,
            timestamp,
        )
    }

    /// Sets the unit filter to the saved filter with the given ID.
    pub fn set_filter(&mut self, filter_id: &str) -> Result<()> {
        ensure!(self.trane.is_some(), "no Trane instance is open");

        let named_filter = self
            .trane
            .as_ref()
            .unwrap()
            .get_filter(filter_id)
            .ok_or_else(|| anyhow!("no filter with ID {}", filter_id))?;
        self.filter = Some(named_filter.filter);
        Ok(())
    }

    /// Shows the answer to the current exercise.
    pub fn show_answer(&mut self) -> Result<()> {
        ensure!(self.trane.is_some(), "no Trane instance is open");

        let curr_exercise = self.current_exercise()?;
        curr_exercise.1.display_answer()
    }

    /// Shows all the entries in the blacklist.
    pub fn show_blacklist(&self) -> Result<()> {
        ensure!(self.trane.is_some(), "no Trane instance is open");

        let entries = self.trane.as_ref().unwrap().all_entries()?;

        if entries.is_empty() {
            println!("No entries in the blacklist");
            return Ok(());
        }
        println!("Blacklist:");
        for entry in entries {
            println!("Unit ID: {}", entry);
        }
        Ok(())
    }

    pub fn show_filter(&self) {
        if self.filter.is_none() {
            println!("No filter is set");
        } else {
            println!("Filter:");
            println!("{:#?}", self.filter.as_ref().unwrap());
        }
    }

    pub fn show_course_instructions(&self, course_id: &str) -> Result<()> {
        ensure!(self.trane.is_some(), "no Trane instance is open");

        let course_id = self.course_id_or_current(course_id)?;
        let manifest = self
            .trane
            .as_ref()
            .unwrap()
            .get_course_manifest(&course_id)
            .ok_or_else(|| anyhow!("no manifest for course with ID {}", course_id))?;
        match manifest.course_instructions {
            None => {
                println!("Course has no instructions");
                Ok(())
            }
            Some(instructions) => instructions.display_asset(),
        }
    }

    pub fn show_lesson_instructions(&self, lesson_id: &str) -> Result<()> {
        ensure!(self.trane.is_some(), "no Trane instance is open");

        let lesson_id = self.lesson_id_or_current(lesson_id)?;
        let manifest = self
            .trane
            .as_ref()
            .unwrap()
            .get_lesson_manifest(&lesson_id)
            .ok_or_else(|| anyhow!("no manifest for lesson with ID {}", lesson_id))?;
        match manifest.lesson_instructions {
            None => {
                println!("Lesson has no instructions");
                Ok(())
            }
            Some(instructions) => instructions.display_asset(),
        }
    }

    pub fn show_course_material(&self, course_id: &str) -> Result<()> {
        ensure!(self.trane.is_some(), "no Trane instance is open");

        let course_id = self.course_id_or_current(course_id)?;
        let manifest = self
            .trane
            .as_ref()
            .unwrap()
            .get_course_manifest(&course_id)
            .ok_or_else(|| anyhow!("no manifest for course with ID {}", course_id))?;
        match manifest.course_material {
            None => {
                println!("Course has no material");
                Ok(())
            }
            Some(material) => material.display_asset(),
        }
    }

    pub fn show_lesson_material(&self, lesson_id: &str) -> Result<()> {
        ensure!(self.trane.is_some(), "no Trane instance is open");

        let lesson_id = self.lesson_id_or_current(lesson_id)?;
        let manifest = self
            .trane
            .as_ref()
            .unwrap()
            .get_lesson_manifest(&lesson_id)
            .ok_or_else(|| anyhow!("no manifest for lesson with ID {}", lesson_id))?;
        match manifest.lesson_material {
            None => {
                println!("Lesson has no material");
                Ok(())
            }
            Some(material) => material.display_asset(),
        }
    }

    /// Prints the manifest for the unit with the given UID.
    fn show_unit_manifest(&self, unit_id: &str, unit_type: UnitType) -> Result<()> {
        ensure!(self.trane.is_some(), "no Trane instance is open");

        match unit_type {
            UnitType::Exercise => {
                let manifest = self
                    .trane
                    .as_ref()
                    .unwrap()
                    .get_exercise_manifest(unit_id)
                    .ok_or_else(|| anyhow!("missing manifest for exercise {}", unit_id))?;
                println!("Unit manifest:");
                println!("{:#?}", manifest);
            }
            UnitType::Lesson => {
                let manifest = self
                    .trane
                    .as_ref()
                    .unwrap()
                    .get_lesson_manifest(unit_id)
                    .ok_or_else(|| anyhow!("missing manifest for lesson {}", unit_id))?;
                println!("Unit manifest:");
                println!("{:#?}", manifest);
            }
            UnitType::Course => {
                let manifest = self
                    .trane
                    .as_ref()
                    .unwrap()
                    .get_course_manifest(unit_id)
                    .ok_or_else(|| anyhow!("missing manifest for course {}", unit_id))?;
                println!("Unit manifest:");
                println!("{:#?}", manifest);
            }
        };
        Ok(())
    }

    /// Prints information about the given unit.
    pub fn show_unit_info(&self, unit_id: &str) -> Result<()> {
        ensure!(self.trane.is_some(), "no Trane instance is open");

        let unit_uid = self.get_unit_uid(unit_id)?;
        let unit_type = self.get_unit_type(unit_id)?;
        println!("Unit ID: {}", unit_id);
        println!("Unit UID: {}", unit_uid);
        println!("Unit type: {:?}", unit_type);
        self.show_unit_manifest(unit_id, unit_type)
    }

    /// Removes the given unit from the blacklist.
    pub fn whitelist(&mut self, unit_id: &str) -> Result<()> {
        ensure!(self.trane.is_some(), "no Trane instance is open");

        self.trane.as_mut().unwrap().remove_unit(unit_id)?;
        self.reset_batch();
        Ok(())
    }
}